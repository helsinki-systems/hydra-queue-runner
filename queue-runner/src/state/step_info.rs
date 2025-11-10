use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use db::models::BuildID;
use nix_utils::BaseStore as _;

use super::build::Step;

pub struct StepInfo {
    pub step: Arc<Step>,
    pub resolved_drv_path: Option<nix_utils::StorePath>,
    already_scheduled: AtomicBool,
    cancelled: AtomicBool,
    pub runnable_since: jiff::Timestamp,
    lowest_share_used: atomic_float::AtomicF64,
}

impl StepInfo {
    pub async fn new(store: &nix_utils::LocalStore, step: Arc<Step>) -> Self {
        Self {
            resolved_drv_path: store.try_resolve_drv(step.get_drv_path()).await,
            already_scheduled: false.into(),
            cancelled: false.into(),
            runnable_since: step.get_runnable_since(),
            lowest_share_used: step.get_lowest_share_used().into(),
            step,
        }
    }

    pub fn update_internal_stats(&self) {
        self.lowest_share_used
            .store(self.step.get_lowest_share_used(), Ordering::Relaxed);
    }

    pub fn get_lowest_share_used(&self) -> f64 {
        self.lowest_share_used.load(Ordering::Relaxed)
    }

    pub fn get_highest_global_priority(&self) -> i32 {
        self.step
            .atomic_state
            .highest_global_priority
            .load(Ordering::Relaxed)
    }

    pub fn get_highest_local_priority(&self) -> i32 {
        self.step
            .atomic_state
            .highest_local_priority
            .load(Ordering::Relaxed)
    }

    pub fn get_lowest_build_id(&self) -> BuildID {
        self.step
            .atomic_state
            .lowest_build_id
            .load(Ordering::Relaxed)
    }

    pub fn get_already_scheduled(&self) -> bool {
        self.already_scheduled.load(Ordering::SeqCst)
    }

    pub fn set_already_scheduled(&self, v: bool) {
        self.already_scheduled.store(v, Ordering::SeqCst);
    }

    pub fn set_cancelled(&self, v: bool) {
        self.cancelled.store(v, Ordering::SeqCst);
    }

    pub fn get_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub(super) fn legacy_compare(&self, other: &StepInfo) -> std::cmp::Ordering {
        #[allow(irrefutable_let_patterns)]
        (if let c1 = self
            .get_highest_global_priority()
            .cmp(&other.get_highest_global_priority())
            && c1 != std::cmp::Ordering::Equal
        {
            c1
        } else if let c2 = other
            .get_lowest_share_used()
            .total_cmp(&self.get_lowest_share_used())
            && c2 != std::cmp::Ordering::Equal
        {
            c2
        } else if let c3 = self
            .get_highest_local_priority()
            .cmp(&other.get_highest_local_priority())
            && c3 != std::cmp::Ordering::Equal
        {
            c3
        } else {
            other.get_lowest_build_id().cmp(&self.get_lowest_build_id())
        })
        .reverse()
    }

    pub(super) fn compare_with_rdeps(&self, other: &StepInfo) -> std::cmp::Ordering {
        #[allow(irrefutable_let_patterns)]
        (if let c1 = self
            .get_highest_global_priority()
            .cmp(&other.get_highest_global_priority())
            && c1 != std::cmp::Ordering::Equal
        {
            c1
        } else if let c2 = other
            .get_lowest_share_used()
            .total_cmp(&self.get_lowest_share_used())
            && c2 != std::cmp::Ordering::Equal
        {
            c2
        } else if let c3 = self
            .step
            .atomic_state
            .rdeps_len
            .load(Ordering::Relaxed)
            .cmp(&other.step.atomic_state.rdeps_len.load(Ordering::Relaxed))
            && c3 != std::cmp::Ordering::Equal
        {
            c3
        } else if let c4 = self
            .get_highest_local_priority()
            .cmp(&other.get_highest_local_priority())
            && c4 != std::cmp::Ordering::Equal
        {
            c4
        } else {
            other.get_lowest_build_id().cmp(&self.get_lowest_build_id())
        })
        .reverse()
    }
}
