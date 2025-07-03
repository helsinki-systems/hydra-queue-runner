#![allow(dead_code)]

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering},
    },
};

use ahash::{AHashMap, AHashSet};
use chrono::TimeZone;

use super::jobset::{Jobset, JobsetID};
use crate::db::models::BuildStatus;

pub type BuildID = i32;
pub type AtomicBuildID = AtomicI32;
pub type StorePath = String;

#[derive(Debug)]
pub struct Build {
    pub id: BuildID,
    pub drv_path: StorePath,
    pub outputs: HashMap<String, StorePath>,
    pub jobset_id: JobsetID,
    pub name: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub max_silent_time: i32,
    pub timeout: i32,
    pub local_priority: i32,
    pub global_priority: AtomicI32,

    pub toplevel: parking_lot::RwLock<Option<Arc<Step>>>,
    pub jobset: Arc<Jobset>,

    pub finished_in_db: AtomicBool,
}

impl PartialEq for Build {
    fn eq(&self, other: &Self) -> bool {
        self.drv_path == other.drv_path
    }
}

impl Eq for Build {}

impl std::hash::Hash for Build {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // ensure that drv_path is never mutable
        // as we set Build as ignore-interior-mutability
        self.drv_path.hash(state);
    }
}

impl Build {
    pub fn new_debug(drv_path: &StorePath) -> Arc<Self> {
        Arc::new(Self {
            id: BuildID::MAX,
            drv_path: drv_path.to_owned(),
            outputs: HashMap::new(),
            jobset_id: JobsetID::MAX,
            name: "debug".into(),
            timestamp: chrono::Utc::now(),
            max_silent_time: i32::MAX,
            timeout: i32::MAX,
            local_priority: 1000,
            global_priority: 1000.into(),
            toplevel: parking_lot::RwLock::new(None),
            jobset: Arc::new(Jobset::new(JobsetID::MAX, "debug", "debug")),
            finished_in_db: false.into(),
        })
    }

    #[tracing::instrument(skip(v, jobset), err)]
    pub fn new(v: crate::db::models::Build, jobset: Arc<Jobset>) -> anyhow::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            id: v.id,
            drv_path: v.drvpath,
            outputs: HashMap::new(),
            jobset_id: v.jobset_id,
            name: v.job,
            timestamp: chrono::Utc.timestamp_opt(v.timestamp, 0).single().ok_or(
                anyhow::anyhow!("Failed to convert unix timestamp into chrono::UTC"),
            )?,
            max_silent_time: v.maxsilent.unwrap_or(3600),
            timeout: v.timeout.unwrap_or(36000),
            local_priority: v.priority,
            global_priority: v.globalpriority.into(),
            toplevel: parking_lot::RwLock::new(None),
            jobset,
            finished_in_db: false.into(),
        }))
    }

    pub fn full_job_name(&self) -> String {
        format!(
            "{}:{}:{}",
            self.jobset.project_name, self.jobset.name, self.name
        )
    }

    #[tracing::instrument(skip(self, step))]
    pub fn set_toplevel_step(&self, step: Arc<Step>) {
        let mut toplevel = self.toplevel.write();
        *toplevel = Some(step);
    }

    #[allow(clippy::unused_self)]
    pub fn propagate_priorities(&self) {}
}

#[derive(Debug)]
pub struct StepAtomicState {
    pub created: AtomicBool, // Whether the step has finished initialisation.
    pub tries: AtomicU32,    // Number of times we've tried this step.
    pub highest_global_priority: AtomicI32, // The highest global priority of any build depending on this step.
    pub highest_local_priority: AtomicI32, // The highest local priority of any build depending on this step.

    pub lowest_build_id: AtomicBuildID, // The lowest ID of any build depending on this step.
}

impl StepAtomicState {
    pub fn new() -> Self {
        Self {
            created: false.into(),
            tries: 0.into(),
            highest_global_priority: 0.into(),
            highest_local_priority: 0.into(),
            lowest_build_id: BuildID::MAX.into(),
        }
    }
}

#[derive(Debug)]
pub struct StepState {
    pub deps: HashSet<Arc<Step>>, // The build steps on which this step depends.
    pub rdeps: Vec<Weak<Step>>,   // The build steps that depend on this step.
    pub builds: Vec<Weak<Build>>, // Builds that have this step as the top-level derivation.
    pub jobsets: Vec<Arc<Jobset>>, // Jobsets to which this step belongs. Used for determining scheduling priority.
    pub after: chrono::DateTime<chrono::Utc>, // Point in time after which the step can be retried.

    pub runnable_since: chrono::DateTime<chrono::Utc>, // The time at which this step became runnable.
    pub last_supported: chrono::DateTime<chrono::Utc>, // The time that we last saw a machine that supports this step
}

impl StepState {
    pub fn new(
        after: chrono::DateTime<chrono::Utc>,
        runnable_since: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        Self {
            deps: HashSet::new(),
            rdeps: Vec::new(),
            builds: Vec::new(),
            jobsets: Vec::new(),
            after,
            runnable_since,
            last_supported: chrono::Utc::now(),
        }
    }
}

#[derive(Debug)]
pub struct Step {
    drv_path: StorePath,
    drv: arc_swap::ArcSwapOption<nix_utils::Derivation>,

    finished: AtomicBool,
    pub atomic_state: StepAtomicState,
    pub state: parking_lot::RwLock<StepState>,
}

impl PartialEq for Step {
    fn eq(&self, other: &Self) -> bool {
        self.drv_path == other.drv_path
    }
}

impl Eq for Step {}

impl std::hash::Hash for Step {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // ensure that drv_path is never mutable
        // as we set Step as ignore-interior-mutability
        self.drv_path.hash(state);
    }
}

impl Step {
    pub fn new(drv_path: StorePath) -> Arc<Self> {
        Arc::new(Self {
            drv_path,
            drv: arc_swap::ArcSwapOption::from(None),
            finished: false.into(),
            atomic_state: StepAtomicState::new(),
            state: parking_lot::RwLock::new(StepState::new(
                chrono::DateTime::<chrono::Utc>::MIN_UTC,
                chrono::DateTime::<chrono::Utc>::MIN_UTC,
            )),
        })
    }

    pub fn get_drv_path(&self) -> &StorePath {
        &self.drv_path
    }

    pub fn get_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    pub fn set_finished(&self, v: bool) {
        self.finished.store(v, Ordering::SeqCst);
    }

    pub fn set_drv(&self, drv: nix_utils::Derivation) {
        self.drv.store(Some(Arc::new(drv)));
    }

    pub fn get_system(&self) -> Option<String> {
        let drv = self.drv.load_full();
        drv.as_ref().map(|drv| drv.system.clone())
    }

    pub fn get_inputs(&self) -> Option<Vec<String>> {
        let drv = self.drv.load_full();
        drv.as_ref().map(|drv| drv.input_drvs.clone())
    }

    pub fn get_outputs(&self) -> Option<Vec<nix_utils::DerivationOutput>> {
        let drv = self.drv.load_full();
        drv.as_ref().map(|drv| drv.outputs.clone())
    }

    #[tracing::instrument(skip(self, builds, steps))]
    pub fn get_dependents(
        self: &Arc<Self>,
        builds: &mut AHashSet<Arc<Build>>,
        steps: &mut AHashSet<Arc<Step>>,
    ) {
        if steps.contains(self) {
            return;
        }
        steps.insert(self.clone());

        let rdeps = {
            let state = self.state.read();
            for b in &state.builds {
                let Some(b) = b.upgrade() else { continue };

                if !b.finished_in_db.load(Ordering::SeqCst) {
                    builds.insert(b);
                }
            }
            state.rdeps.clone()
        };

        for rdep in rdeps {
            let Some(rdep) = rdep.upgrade() else { continue };
            rdep.get_dependents(builds, steps);
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct RemoteBuild {
    pub step_status: BuildStatus,
    pub can_retry: bool,           // for bsAborted
    pub is_cached: bool,           // for bsSucceed
    pub can_cache: bool,           // for bsFailed
    pub error_msg: Option<String>, // for bsAborted

    pub times_build: i32,
    pub is_non_deterministic: bool,

    pub start_time: i64,
    pub stop_time: i64,

    pub overhead: i32,
    pub log_file: String,
}

impl RemoteBuild {
    pub fn new() -> Self {
        Self {
            step_status: BuildStatus::Aborted,
            can_retry: false,
            is_cached: false,
            can_cache: false,
            error_msg: None,
            times_build: 0,
            is_non_deterministic: false,
            start_time: 0,
            stop_time: 0,
            overhead: 0,
            log_file: String::new(),
        }
    }
}

pub struct BuildProduct {
    pub path: String,
    pub default_path: String,

    pub r#type: String,
    pub subtype: String,
    pub name: String,

    pub is_regular: bool,

    pub sha256hash: Option<String>,
    pub file_size: Option<u64>,
}

impl From<crate::db::models::BuildProduct> for BuildProduct {
    fn from(v: crate::db::models::BuildProduct) -> Self {
        Self {
            path: v.path.unwrap_or_default(),
            default_path: v.defaultpath.unwrap_or_default(),
            r#type: v.r#type,
            subtype: v.subtype,
            name: v.name,
            is_regular: v.filesize.is_some(),
            sha256hash: v.sha256hash,
            #[allow(clippy::cast_sign_loss)]
            file_size: v.filesize.map(|v| v as u64),
        }
    }
}

impl From<crate::server::grpc::runner_v1::BuildProduct> for BuildProduct {
    fn from(v: crate::server::grpc::runner_v1::BuildProduct) -> Self {
        Self {
            path: v.path,
            default_path: v.default_path,
            r#type: v.r#type,
            subtype: v.subtype,
            name: v.name,
            is_regular: v.is_regular,
            sha256hash: v.sha256hash,
            file_size: v.file_size,
        }
    }
}

impl From<nix_utils::BuildProduct> for BuildProduct {
    fn from(v: nix_utils::BuildProduct) -> Self {
        Self {
            path: v.path,
            default_path: v.default_path,
            r#type: v.r#type,
            subtype: v.subtype,
            name: v.name,
            is_regular: v.is_regular,
            sha256hash: v.sha256hash,
            file_size: v.file_size,
        }
    }
}

pub struct BuildMetric {
    pub name: String,
    pub unit: Option<String>,
    pub value: f64,
}

pub struct BuildOutput {
    pub failed: bool,

    pub release_name: Option<String>,

    pub closure_size: u64,
    pub size: u64,

    pub products: Vec<BuildProduct>,
    pub outputs: AHashMap<String, String>,
    pub metrics: AHashMap<String, BuildMetric>,
}

impl TryFrom<crate::db::models::BuildOutput> for BuildOutput {
    type Error = anyhow::Error;

    fn try_from(v: crate::db::models::BuildOutput) -> anyhow::Result<Self> {
        let build_status = BuildStatus::from_i32(
            v.buildstatus
                .ok_or(anyhow::anyhow!("buildstatus missing"))?,
        )
        .ok_or(anyhow::anyhow!("buildstatus did not map"))?;
        Ok(Self {
            failed: build_status != BuildStatus::Success,
            release_name: v.releasename,
            #[allow(clippy::cast_sign_loss)]
            closure_size: v.closuresize.unwrap_or_default() as u64,
            #[allow(clippy::cast_sign_loss)]
            size: v.size.unwrap_or_default() as u64,
            products: vec![],
            outputs: AHashMap::new(),
            metrics: AHashMap::new(),
        })
    }
}

impl From<crate::server::grpc::runner_v1::BuildResultInfo> for BuildOutput {
    fn from(v: crate::server::grpc::runner_v1::BuildResultInfo) -> Self {
        let mut outputs = AHashMap::new();
        let mut closure_size = 0;
        let mut nar_size = 0;

        for o in v.outputs {
            match o.output {
                Some(crate::server::grpc::runner_v1::output::Output::Nameonly(_)) => {
                    // We dont care about outputs that dont have a path,
                }
                Some(crate::server::grpc::runner_v1::output::Output::Withpath(o)) => {
                    outputs.insert(o.name, o.path);
                    closure_size += o.closure_size;
                    nar_size += o.nar_size;
                }
                None => (),
            }
        }
        let (failed, release_name, products, metrics) = if let Some(nix_support) = v.nix_support {
            (
                nix_support.failed,
                nix_support.hydra_release_name,
                nix_support.products,
                nix_support.metrics,
            )
        } else {
            (false, None, vec![], vec![])
        };

        Self {
            failed,
            release_name,
            closure_size,
            size: nar_size,
            products: products.into_iter().map(Into::into).collect(),
            outputs,
            metrics: metrics
                .into_iter()
                .map(|v| {
                    (
                        v.path,
                        BuildMetric {
                            name: v.name,
                            unit: v.unit,
                            value: v.value,
                        },
                    )
                })
                .collect(),
        }
    }
}

impl BuildOutput {
    #[tracing::instrument(skip(outputs))]
    pub async fn new(outputs: Vec<nix_utils::DerivationOutput>) -> anyhow::Result<Self> {
        let flat_outputs = outputs
            .iter()
            .filter_map(|o| o.path.as_deref())
            .collect::<Vec<_>>();
        let pathinfos = nix_utils::query_path_infos(&flat_outputs).await?;
        let nix_support = nix_utils::parse_nix_support_from_outputs(&flat_outputs).await?;

        let mut outputs_map = AHashMap::new();
        let mut closure_size = 0;
        let mut nar_size = 0;

        for o in outputs {
            if let Some(path) = o.path {
                if let Some(info) = pathinfos.get(&path) {
                    outputs_map.insert(o.name, path);
                    closure_size += info.closure_size;
                    nar_size += info.nar_size;
                }
            }
        }

        Ok(Self {
            failed: nix_support.failed,
            release_name: nix_support.hydra_release_name,
            closure_size,
            size: nar_size,
            products: nix_support.products.into_iter().map(Into::into).collect(),
            outputs: outputs_map,
            metrics: nix_support
                .metrics
                .into_iter()
                .map(|v| {
                    (
                        v.path,
                        BuildMetric {
                            name: v.name,
                            unit: v.unit,
                            value: v.value,
                        },
                    )
                })
                .collect(),
        })
    }
}
