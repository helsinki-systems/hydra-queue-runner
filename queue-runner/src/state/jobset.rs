use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicI64, AtomicU32, Ordering},
};

pub type JobsetID = i32;
pub const SCHEDULING_WINDOW: i32 = 24 * 60 * 60;

#[derive(Debug)]
pub struct Jobset {
    pub id: JobsetID,
    pub project_name: String,
    pub name: String,

    seconds: AtomicI64,
    shares: AtomicU32,
    // The start time and duration of the most recent build steps.
    steps: parking_lot::RwLock<BTreeMap<i64, i64>>,
}

impl Jobset {
    pub fn new<S: Into<String>>(id: JobsetID, project_name: S, name: S) -> Self {
        Self {
            id,
            project_name: project_name.into(),
            name: name.into(),
            seconds: 0.into(),
            shares: 0.into(),
            steps: parking_lot::RwLock::new(BTreeMap::new()),
        }
    }

    pub fn share_used(&self) -> f64 {
        let seconds = self.seconds.load(Ordering::SeqCst);
        let shares = self.shares.load(Ordering::SeqCst);

        // we dont care about the precision here
        #[allow(clippy::cast_precision_loss)]
        ((seconds as f64) / f64::from(shares))
    }

    pub fn set_shares(&self, shares: i32) -> anyhow::Result<()> {
        debug_assert!(shares > 0);
        self.shares.store(shares.try_into()?, Ordering::SeqCst);
        Ok(())
    }

    pub fn get_shares(&self) -> u32 {
        self.shares.load(Ordering::SeqCst)
    }

    pub fn get_seconds(&self) -> i64 {
        self.seconds.load(Ordering::SeqCst)
    }

    pub fn add_step(&self, start_time: i64, duration: i64) {
        let mut steps = self.steps.write();
        steps.insert(start_time, duration);
        self.seconds.fetch_add(duration, Ordering::SeqCst);
    }
}
