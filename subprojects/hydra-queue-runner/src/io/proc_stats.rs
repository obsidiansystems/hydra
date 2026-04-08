use anyhow::Context as _;

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
pub struct MemoryStats {
    current_bytes: u64,
    peak_bytes: u64,
    swap_current_bytes: u64,
    zswap_current_bytes: u64,
}

impl MemoryStats {
    #[tracing::instrument(err)]
    fn new(cgroups_path: &std::path::Path) -> anyhow::Result<Self> {
        Ok(Self {
            current_bytes: fs_err::read_to_string(cgroups_path.join("memory.current"))?
                .trim()
                .parse()
                .context("memory current parsing failed")?,
            peak_bytes: fs_err::read_to_string(cgroups_path.join("memory.peak"))?
                .trim()
                .parse()
                .context("memory peak parsing failed")?,
            swap_current_bytes: fs_err::read_to_string(cgroups_path.join("memory.swap.current"))?
                .trim()
                .parse()
                .context("swap parsing failed")?,
            zswap_current_bytes: fs_err::read_to_string(cgroups_path.join("memory.zswap.current"))?
                .trim()
                .parse()
                .context("zswap parsing failed")?,
        })
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IoStats {
    total_read_bytes: u64,
    total_write_bytes: u64,
}

impl IoStats {
    #[tracing::instrument(err)]
    fn new(cgroups_path: &std::path::Path) -> anyhow::Result<Self> {
        let mut total_read_bytes: u64 = 0;
        let mut total_write_bytes: u64 = 0;

        let contents = fs_err::read_to_string(cgroups_path.join("io.stat"))?;
        for line in contents.lines() {
            for part in line.split_whitespace() {
                if part.starts_with("rbytes=") {
                    total_read_bytes += part
                        .split('=')
                        .nth(1)
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                } else if part.starts_with("wbytes=") {
                    total_write_bytes += part
                        .split('=')
                        .nth(1)
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                }
            }
        }

        Ok(Self {
            total_read_bytes,
            total_write_bytes,
        })
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
pub struct CpuStats {
    usage_usec: u128,
    user_usec: u128,
    system_usec: u128,
}

impl CpuStats {
    #[tracing::instrument(err)]
    fn new(cgroups_path: &std::path::Path) -> anyhow::Result<Self> {
        let contents = fs_err::read_to_string(cgroups_path.join("cpu.stat"))?;

        let mut usage_usec: u128 = 0;
        let mut user_usec: u128 = 0;
        let mut system_usec: u128 = 0;

        for line in contents.lines() {
            if line.starts_with("usage_usec") {
                usage_usec = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
            } else if line.starts_with("user_usec") {
                user_usec = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
            } else if line.starts_with("system_usec") {
                system_usec = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
            }
        }
        Ok(Self {
            usage_usec,
            user_usec,
            system_usec,
        })
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CgroupStats {
    memory: MemoryStats,
    io: IoStats,
    cpu: CpuStats,
}

impl CgroupStats {
    #[tracing::instrument(err)]
    fn new(me: &procfs::process::Process) -> anyhow::Result<Self> {
        let cgroups_pathname = format!(
            "/sys/fs/cgroup/{}",
            me.cgroups()?
                .0
                .first()
                .ok_or_else(|| anyhow::anyhow!("cgroup information is missing in process."))?
                .pathname
        );
        let cgroups_path = std::path::Path::new(&cgroups_pathname);
        if !cgroups_path.exists() {
            return Err(anyhow::anyhow!("cgroups directory does not exists."));
        }

        Ok(Self {
            memory: MemoryStats::new(cgroups_path)?,
            io: IoStats::new(cgroups_path)?,
            cpu: CpuStats::new(cgroups_path)?,
        })
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Process {
    pid: i32,
    vsize_bytes: u64,
    rss_bytes: u64,
    shared_bytes: u64,
    cgroup: Option<CgroupStats>,
}

impl Process {
    pub fn new() -> Option<Self> {
        let me = procfs::process::Process::myself().ok()?;
        let page_size = procfs::page_size();
        let statm = me.statm().ok()?;
        let vsize = statm.size * page_size;
        let rss = statm.resident * page_size;
        let shared = statm.shared * page_size;
        Some(Self {
            pid: me.pid,
            vsize_bytes: vsize,
            rss_bytes: rss,
            shared_bytes: shared,
            cgroup: match CgroupStats::new(&me) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::error!("failed to cgroups stats: {e}");
                    None
                }
            },
        })
    }
}
