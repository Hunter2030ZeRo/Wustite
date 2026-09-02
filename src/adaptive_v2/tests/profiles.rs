use super::super::profile::{
    AdaptiveProfile, FactClass, Lifecycle, LiveObservation, ProbeDecision, ProfileCase,
};

fn live(case: u8) -> LiveObservation {
    LiveObservation::new(
        ProfileCase::new(u32::from(case)),
        FactClass::UnknownClassified,
    )
}

#[test]
fn live_profile_requires_exact_32_entry_32_stable_boundaries() {
    let mut profile = AdaptiveProfile::new(7);
    profile.seed_static_hint(ProfileCase::new(1), 10_000);
    assert_eq!(profile.lifecycle(), Lifecycle::Profiling);
    assert_eq!(profile.live_entries(), 0);
    for _ in 0..31 {
        profile.observe_live(live(1));
    }
    assert_eq!(profile.lifecycle(), Lifecycle::Profiling);
    profile.observe_live(live(1));
    assert_eq!(profile.lifecycle(), Lifecycle::ReadyToRecord);
    assert!(profile.start_recording());
    assert_eq!(profile.lifecycle(), Lifecycle::Recording);
    assert!(profile.finish_recording());
    for _ in 0..31 {
        profile.observe_live(live(1));
    }
    assert_eq!(profile.lifecycle(), Lifecycle::Recording);
    profile.observe_live(live(1));
    assert_eq!(profile.lifecycle(), Lifecycle::ReadyToCompile);
    let permit = profile.take_compile_permit().expect("live compile permit");
    assert_eq!(permit.schema_epoch(), 7);
    assert_eq!(profile.lifecycle(), Lifecycle::Compiled);
    assert!(profile.take_compile_permit().is_none());
}

#[test]
fn polymorphism_guardability_proven_facts_obey_live_global_gates() {
    let mut poly = AdaptiveProfile::new(1);
    for index in 0..35 {
        poly.observe_live(live((index % 4) as u8));
    }
    assert_eq!(poly.lifecycle(), Lifecycle::ReadyToRecord);
    assert_eq!(poly.case_count(), 4);

    let mut generic = AdaptiveProfile::new(1);
    for case in 0..5 {
        generic.observe_live(live(case));
    }
    for _ in 0..31 {
        generic.observe_live(live(99));
    }
    assert!(generic.is_generic());
    assert_eq!(generic.lifecycle(), Lifecycle::ReadyToRecord);
    assert!(generic.take_record_permit().is_none());
    assert!(generic.take_compile_permit().is_none());

    let mut guarded = AdaptiveProfile::new(1);
    for _ in 0..128 {
        let decision = guarded.observe_live(LiveObservation::new(
            ProfileCase::new(3),
            FactClass::Guardable {
                guard_emitted: false,
                live_confirmed: false,
            },
        ));
        assert_eq!(decision, ProbeDecision::LiveProbe);
    }
    assert_eq!(guarded.lifecycle(), Lifecycle::Profiling);

    let mut proven = AdaptiveProfile::new(1);
    for _ in 0..31 {
        assert_eq!(
            proven.observe_live(LiveObservation::new(ProfileCase::new(4), FactClass::Proven,)),
            ProbeDecision::ElidedProven
        );
    }
    assert_eq!(proven.lifecycle(), Lifecycle::Profiling);
    proven.observe_live(LiveObservation::new(ProfileCase::new(4), FactClass::Proven));
    assert_eq!(proven.lifecycle(), Lifecycle::ReadyToRecord);
}

#[test]
fn invalidation_resets_live_ready_rejects_stale_schema_evidence() {
    let mut profile = AdaptiveProfile::new(4);
    for _ in 0..32 {
        profile.observe_live(live(1));
    }
    assert_eq!(profile.lifecycle(), Lifecycle::ReadyToRecord);
    profile.invalidate(5);
    assert_eq!(profile.lifecycle(), Lifecycle::Profiling);
    assert_eq!(profile.live_entries(), 0);
    assert_eq!(profile.schema_epoch(), 5);
    for _ in 0..32 {
        profile.observe_live(live(1));
    }
    assert_eq!(profile.lifecycle(), Lifecycle::ReadyToRecord);
}

#[test]
fn stable_polymorphic_boundary_illegal_transitions_explicit() {
    let mut profile = AdaptiveProfile::new(9);
    assert!(!profile.start_recording());
    assert!(!profile.finish_recording());
    for case in 0..4 {
        profile.observe_live(live(case));
    }
    for index in 0..27 {
        profile.observe_live(live((index % 4) as u8));
    }
    assert_eq!(profile.stable_live(), 28);
    assert_eq!(profile.live_entries(), 31);
    assert_eq!(profile.lifecycle(), Lifecycle::Profiling);
    profile.observe_live(live(0));
    assert_eq!(profile.live_entries(), 32);
    assert_eq!(profile.stable_live(), 29);
    assert_eq!(profile.lifecycle(), Lifecycle::Profiling);
    for case in 1..4 {
        profile.observe_live(live(case));
    }
    assert_eq!(profile.live_entries(), 35);
    assert_eq!(profile.stable_live(), 32);
    assert_eq!(profile.lifecycle(), Lifecycle::ReadyToRecord);
    assert!(profile.start_recording());
    assert!(!profile.start_recording());
    assert!(profile.finish_recording());
    assert!(!profile.finish_recording());
}
