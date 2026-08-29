use crate::planner::JitPolicy;

use super::Runtime;

impl Runtime {
    pub fn set_jit_policy(&mut self, policy: JitPolicy) {
        self.vm.set_jit_policy(policy);
    }

    pub fn set_dump_wxir(&mut self, enabled: bool) {
        self.vm.set_dump_wxir(enabled);
    }
}
