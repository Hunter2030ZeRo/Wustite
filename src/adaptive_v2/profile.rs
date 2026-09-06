#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProfileCase(u32);

impl ProfileCase {
    pub(crate) const fn new(value: u32) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FactClass {
    Proven,
    Guardable {
        guard_emitted: bool,
        live_confirmed: bool,
    },
    UnknownClassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeDecision {
    ElidedProven,
    Guarded,
    LiveProbe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveObservation {
    case: ProfileCase,
    fact: FactClass,
}

impl LiveObservation {
    pub(crate) const fn new(case: ProfileCase, fact: FactClass) -> Self {
        Self { case, fact }
    }

    pub(crate) const fn case(self) -> ProfileCase {
        self.case
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lifecycle {
    Profiling,
    ReadyToRecord,
    Recording,
    ReadyToCompile,
    Compiled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompilePermit {
    schema_epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecordPermit {
    schema_epoch: u64,
}

impl RecordPermit {
    pub(crate) const fn schema_epoch(self) -> u64 {
        self.schema_epoch
    }
}

impl CompilePermit {
    pub(crate) const fn schema_epoch(self) -> u64 {
        self.schema_epoch
    }
}

#[derive(Debug)]
pub(crate) struct AdaptiveProfile {
    lifecycle: Lifecycle,
    schema_epoch: u64,
    hot_threshold: u64,
    live_entries: u64,
    stable_live: u64,
    cases: Vec<ProfileCase>,
    generic: bool,
    static_hints: Vec<ProfileCase>,
    recording_complete: bool,
}

impl AdaptiveProfile {
    pub(crate) const fn new(schema_epoch: u64, hot_threshold: u64) -> Self {
        Self {
            lifecycle: Lifecycle::Profiling,
            schema_epoch,
            hot_threshold: if hot_threshold == 0 { 1 } else { hot_threshold },
            live_entries: 0,
            stable_live: 0,
            cases: Vec::new(),
            generic: false,
            static_hints: Vec::new(),
            recording_complete: false,
        }
    }

    pub(crate) const fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    pub(crate) const fn schema_epoch(&self) -> u64 {
        self.schema_epoch
    }

    pub(crate) const fn live_entries(&self) -> u64 {
        self.live_entries
    }

    pub(crate) const fn stable_live(&self) -> u64 {
        self.stable_live
    }

    pub(crate) fn case_count(&self) -> usize {
        self.cases.len()
    }

    pub(crate) const fn is_generic(&self) -> bool {
        self.generic
    }

    pub(crate) fn seed_static_hint(&mut self, case: ProfileCase, _count: u64) {
        if !self.static_hints.contains(&case) {
            self.static_hints.push(case);
        }
    }

    pub(crate) fn observe_live(&mut self, observation: LiveObservation) -> ProbeDecision {
        let decision = match observation.fact {
            FactClass::Proven => ProbeDecision::ElidedProven,
            FactClass::Guardable {
                guard_emitted: true,
                live_confirmed: true,
            } => ProbeDecision::Guarded,
            FactClass::Guardable {
                guard_emitted: _,
                live_confirmed: _,
            }
            | FactClass::UnknownClassified => ProbeDecision::LiveProbe,
        };
        self.live_entries = self.live_entries.saturating_add(1);
        let guard_valid = !matches!(
            observation.fact,
            FactClass::Guardable {
                guard_emitted: false,
                live_confirmed: _,
            } | FactClass::Guardable {
                guard_emitted: _,
                live_confirmed: false,
            }
        );
        if !guard_valid {
            self.stable_live = 0;
            return decision;
        }
        self.observe_case(observation.case);
        match self.lifecycle {
            Lifecycle::Profiling
                if self.live_entries >= self.hot_threshold
                    && self.stable_live >= self.hot_threshold =>
            {
                self.lifecycle = Lifecycle::ReadyToRecord;
            }
            Lifecycle::Recording
                if self.recording_complete && self.stable_live >= self.hot_threshold =>
            {
                self.lifecycle = Lifecycle::ReadyToCompile;
            }
            Lifecycle::Profiling
            | Lifecycle::ReadyToRecord
            | Lifecycle::Recording
            | Lifecycle::ReadyToCompile
            | Lifecycle::Compiled => {}
        }
        decision
    }

    pub(crate) fn start_recording(&mut self) -> bool {
        if self.lifecycle != Lifecycle::ReadyToRecord {
            return false;
        }
        self.lifecycle = Lifecycle::Recording;
        self.stable_live = 0;
        self.recording_complete = false;
        true
    }

    pub(crate) fn take_record_permit(&mut self) -> Option<RecordPermit> {
        if self.generic || !self.start_recording() {
            return None;
        }
        Some(RecordPermit {
            schema_epoch: self.schema_epoch,
        })
    }

    pub(crate) fn finish_recording(&mut self) -> bool {
        if self.lifecycle != Lifecycle::Recording || self.recording_complete {
            return false;
        }
        self.recording_complete = true;
        self.stable_live = 0;
        true
    }

    pub(crate) fn take_compile_permit(&mut self) -> Option<CompilePermit> {
        if self.generic || self.lifecycle != Lifecycle::ReadyToCompile {
            return None;
        }
        self.lifecycle = Lifecycle::Compiled;
        Some(CompilePermit {
            schema_epoch: self.schema_epoch,
        })
    }

    pub(crate) fn invalidate(&mut self, schema_epoch: u64) {
        self.lifecycle = Lifecycle::Profiling;
        self.schema_epoch = schema_epoch;
        self.live_entries = 0;
        self.stable_live = 0;
        self.cases.clear();
        self.generic = false;
        self.recording_complete = false;
    }

    fn observe_case(&mut self, case: ProfileCase) {
        if self.generic {
            self.stable_live = self.stable_live.saturating_add(1);
            return;
        }
        if self.cases.contains(&case) {
            self.stable_live = self.stable_live.saturating_add(1);
            return;
        }
        if self.cases.len() == 4 {
            self.generic = true;
            self.stable_live = 1;
            return;
        }
        self.cases.push(case);
        self.stable_live = 1;
    }
}
