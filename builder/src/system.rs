use procfs::Current as _;

pub struct BaseSystemInfo {
    pub cpu_count: usize,
    pub bogomips: f32,
    pub total_memory: u64,
}

impl BaseSystemInfo {
    pub fn new() -> anyhow::Result<Self> {
        let cpuinfo = procfs::CpuInfo::current()?;
        let meminfo = procfs::Meminfo::current()?;
        let bogomips = cpuinfo
            .fields
            .get("bogomips")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);

        Ok(Self {
            cpu_count: cpuinfo.num_cores(),
            bogomips,
            total_memory: meminfo.mem_total,
        })
    }
}

pub struct Pressure {
    pub avg10: f32,
    pub avg60: f32,
    pub avg300: f32,
    pub total: u64,
}

impl Pressure {
    fn new(record: &procfs::PressureRecord) -> Self {
        Self {
            avg10: record.avg10,
            avg60: record.avg60,
            avg300: record.avg300,
            total: record.total,
        }
    }
}

impl From<Pressure> for crate::runner_v1::Pressure {
    fn from(val: Pressure) -> Self {
        Self {
            avg10: val.avg10,
            avg60: val.avg60,
            avg300: val.avg300,
            total: val.total,
        }
    }
}

pub struct PressureState {
    pub cpu_some: Pressure,
    pub mem_some: Pressure,
    pub mem_full: Pressure,
    pub io_some: Pressure,
    pub io_full: Pressure,
}

impl PressureState {
    pub fn new() -> Option<Self> {
        let cpu_psi = procfs::CpuPressure::current().ok()?;
        let mem_psi = procfs::MemoryPressure::current().ok()?;
        let io_psi = procfs::IoPressure::current().ok()?;

        Some(Self {
            cpu_some: Pressure::new(&cpu_psi.some),
            mem_some: Pressure::new(&mem_psi.some),
            mem_full: Pressure::new(&mem_psi.full),
            io_some: Pressure::new(&io_psi.some),
            io_full: Pressure::new(&io_psi.full),
        })
    }
}

pub struct SystemLoad {
    pub load_avg_1: f32,
    pub load_avg_5: f32,
    pub load_avg_15: f32,

    pub mem_usage: u64,
    pub pressure: Option<PressureState>,

    pub tmp_free_percent: f64,
    pub store_free_percent: f64,
}

pub fn get_mount_free_percent(dest: &str) -> anyhow::Result<f64> {
    let stat = nix::sys::statvfs::statvfs(dest)?;

    let total_bytes = stat.blocks() * stat.block_size();
    let free_bytes = stat.blocks_available() * stat.block_size();
    #[allow(clippy::cast_precision_loss)]
    Ok(free_bytes as f64 / total_bytes as f64 * 100.0)
}

impl SystemLoad {
    pub fn new() -> anyhow::Result<Self> {
        let meminfo = procfs::Meminfo::current()?;
        let load = procfs::LoadAverage::current()?;

        // TODO: prefix
        let nix_state_dir = std::env::var("NIX_STORE_DIR").unwrap_or("/nix/store".to_owned());

        Ok(Self {
            load_avg_1: load.one,
            load_avg_5: load.five,
            load_avg_15: load.fifteen,
            mem_usage: meminfo.mem_total - meminfo.mem_available.unwrap_or(0),
            pressure: PressureState::new(),
            tmp_free_percent: get_mount_free_percent("/tmp").unwrap_or(0.),
            store_free_percent: get_mount_free_percent(&nix_state_dir).unwrap_or(0.),
        })
    }
}
