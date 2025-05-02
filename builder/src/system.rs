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

pub struct SystemLoad {
    pub load_avg_1: f32,
    pub load_avg_5: f32,
    pub load_avg_15: f32,

    pub mem_usage: u64,
    pub cpu_some_psi: Pressure,
    pub mem_some_psi: Pressure,
    pub mem_full_psi: Pressure,
    pub io_some_psi: Pressure,
    pub io_full_psi: Pressure,
}

impl SystemLoad {
    pub fn new() -> anyhow::Result<Self> {
        let meminfo = procfs::Meminfo::current()?;
        let load = procfs::LoadAverage::current()?;
        let cpu_psi = procfs::CpuPressure::current()?;
        let mem_psi = procfs::MemoryPressure::current()?;
        let io_psi = procfs::IoPressure::current()?;

        Ok(Self {
            load_avg_1: load.one,
            load_avg_5: load.five,
            load_avg_15: load.fifteen,
            mem_usage: meminfo.mem_total - meminfo.mem_available.unwrap_or(0),
            cpu_some_psi: Pressure::new(&cpu_psi.some),
            mem_some_psi: Pressure::new(&mem_psi.some),
            mem_full_psi: Pressure::new(&mem_psi.full),
            io_some_psi: Pressure::new(&io_psi.some),
            io_full_psi: Pressure::new(&io_psi.full),
        })
    }
}
