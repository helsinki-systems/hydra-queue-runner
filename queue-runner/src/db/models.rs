#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    Success = 0,
    Failed = 1,
    DepFailed = 2, // builds only
    Aborted = 3,
    Cancelled = 4,
    FailedWithOutput = 6, // builds only
    TimedOut = 7,
    CachedFailure = 8, // steps only
    Unsupported = 9,
    LogLimitExceeded = 10,
    NarSizeLimitExceeded = 11,
    NotDeterministic = 12,
    Busy = 100, // not stored
}

impl BuildStatus {
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::Success),
            1 => Some(Self::Failed),
            2 => Some(Self::DepFailed),
            3 => Some(Self::Aborted),
            4 => Some(Self::Cancelled),
            6 => Some(Self::FailedWithOutput),
            7 => Some(Self::TimedOut),
            8 => Some(Self::CachedFailure),
            9 => Some(Self::Unsupported),
            10 => Some(Self::LogLimitExceeded),
            11 => Some(Self::NarSizeLimitExceeded),
            12 => Some(Self::NotDeterministic),
            100 => Some(Self::Busy),
            _ => None,
        }
    }
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum StepStatus {
    Preparing = 1,
    Connecting = 10,
    SendingInputs = 20,
    Building = 30,
    WaitingForLocalSlot = 35,
    ReceivingOutputs = 40,
    PostProcessing = 50,
}

impl From<crate::server::grpc::runner_v1::StepStatus> for StepStatus {
    fn from(item: crate::server::grpc::runner_v1::StepStatus) -> Self {
        match item {
            crate::server::grpc::runner_v1::StepStatus::Preparing => Self::Preparing,
            crate::server::grpc::runner_v1::StepStatus::Connecting => Self::Connecting,
            crate::server::grpc::runner_v1::StepStatus::SeningInputs => Self::SendingInputs,
            crate::server::grpc::runner_v1::StepStatus::Building => Self::Building,
            crate::server::grpc::runner_v1::StepStatus::WaitingForLocalSlot => {
                Self::WaitingForLocalSlot
            }
            crate::server::grpc::runner_v1::StepStatus::ReceivingOutputs => Self::ReceivingOutputs,
            crate::server::grpc::runner_v1::StepStatus::PostProcessing => Self::PostProcessing,
        }
    }
}

pub struct Jobset {
    pub project: String,
    pub name: String,
    pub schedulingshares: i32,
}

pub struct BuildSmall {
    pub id: i32,
    pub globalpriority: i32,
}

pub struct Build {
    pub id: i32,
    pub jobset_id: i32,
    pub project: String,
    pub jobset: String,
    pub job: String,
    pub drvpath: String,
    pub maxsilent: Option<i32>, // maxsilent integer default 3600
    pub timeout: Option<i32>,   // timeout integer default 36000
    // // pub timestamp: chrono::NaiveDateTime,
    pub timestamp: i64,
    pub globalpriority: i32,
    pub priority: i32,
}

pub struct BuildSteps {
    pub starttime: Option<i32>,
    pub stoptime: Option<i32>,
}

#[repr(i32)]
pub enum BuildType {
    Build = 0,
    Substitution = 1,
}

pub struct UpdateBuild {
    pub status: BuildStatus,
    pub start_time: i32,
    pub stop_time: i32,
    pub size: i64,
    pub closure_size: i64,
    pub release_name: Option<String>,
    pub is_cached_build: bool,
}

pub struct InsertBuildStep<'a> {
    pub build_id: i32,
    pub step_nr: i32,
    pub r#type: BuildType,
    pub drv_path: &'a str,
    pub status: BuildStatus,
    pub busy: bool,
    pub start_time: Option<i32>,
    pub stop_time: Option<i32>,
    pub platform: Option<&'a str>,
    pub propagated_from: Option<i32>,
    pub error_msg: Option<&'a str>,
    pub machine: &'a str,
}

pub struct InsertBuildStepOutput {
    pub build_id: i32,
    pub step_nr: i32,
    pub name: String,
    pub path: Option<String>,
}

pub struct UpdateBuildStep {
    pub build_id: i32,
    pub step_nr: i32,
    pub status: StepStatus,
}

pub struct UpdateBuildStepInFinish<'a> {
    pub build_id: i32,
    pub step_nr: i32,
    pub status: BuildStatus,
    pub error_msg: Option<&'a str>,
    pub start_time: i32,
    pub stop_time: i32,
    pub machine: Option<&'a str>,
    pub overhead: Option<i32>,
    pub times_built: Option<i32>,
    pub is_non_deterministic: Option<bool>,
}

pub struct InsertBuildProduct {
    pub build_id: i32,
    pub product_nr: i32,
    pub r#type: String,
    pub subtype: String,
    pub file_size: Option<i64>,
    pub sha256hash: Option<String>,
    pub path: String,
    pub name: String,
    pub default_path: String,
}

pub struct InsertBuildMetric {
    pub build_id: i32,
    pub name: String,
    pub unit: Option<String>,
    pub value: f64,
    pub project: String,
    pub jobset: String,
    pub job: String,
    pub timestamp: i32,
}

pub struct BuildOutput {
    pub id: i32,
    pub buildstatus: Option<i32>,
    pub releasename: Option<String>,
    pub closuresize: Option<i64>,
    pub size: Option<i64>,
}

pub struct BuildProduct {
    pub r#type: String,
    pub subtype: String,
    pub filesize: Option<i64>,
    pub sha256hash: Option<String>,
    pub path: Option<String>,
    pub name: String,
    pub defaultpath: Option<String>,
}
