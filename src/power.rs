/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{bail, ensure, Error, Result};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::error;

use crate::{path, write_synced};

const GPU_HWMON_PREFIX: &str = "/sys/class/hwmon";
const GPU_HWMON_NAME: &str = "amdgpu";

const GPU_PERFORMANCE_LEVEL_SUFFIX: &str = "device/power_dpm_force_performance_level";
const GPU_CLOCKS_SUFFIX: &str = "device/pp_od_clk_voltage";

const TDP_LIMIT1: &str = "power1_cap";
const TDP_LIMIT2: &str = "power2_cap";

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
pub enum GPUPerformanceLevel {
    Auto = 0,
    Low = 1,
    High = 2,
    Manual = 3,
    ProfilePeak = 4,
}

impl TryFrom<u32> for GPUPerformanceLevel {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == GPUPerformanceLevel::Auto as u32 => Ok(GPUPerformanceLevel::Auto),
            x if x == GPUPerformanceLevel::Low as u32 => Ok(GPUPerformanceLevel::Low),
            x if x == GPUPerformanceLevel::High as u32 => Ok(GPUPerformanceLevel::High),
            x if x == GPUPerformanceLevel::Manual as u32 => Ok(GPUPerformanceLevel::Manual),
            x if x == GPUPerformanceLevel::ProfilePeak as u32 => {
                Ok(GPUPerformanceLevel::ProfilePeak)
            }
            _ => Err("No enum match for value {v}"),
        }
    }
}

impl FromStr for GPUPerformanceLevel {
    type Err = Error;
    fn from_str(input: &str) -> Result<GPUPerformanceLevel, Self::Err> {
        Ok(match input {
            "auto" => GPUPerformanceLevel::Auto,
            "low" => GPUPerformanceLevel::Low,
            "high" => GPUPerformanceLevel::High,
            "manual" => GPUPerformanceLevel::Manual,
            "peak_performance" => GPUPerformanceLevel::ProfilePeak,
            v => bail!("No enum match for value {v}"),
        })
    }
}

impl fmt::Display for GPUPerformanceLevel {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            GPUPerformanceLevel::Auto => write!(f, "auto"),
            GPUPerformanceLevel::Low => write!(f, "low"),
            GPUPerformanceLevel::High => write!(f, "high"),
            GPUPerformanceLevel::Manual => write!(f, "manual"),
            GPUPerformanceLevel::ProfilePeak => write!(f, "peak_performance"),
        }
    }
}

pub(crate) async fn get_gpu_performance_level() -> Result<GPUPerformanceLevel> {
    let base = find_hwmon().await?;
    let level = fs::read_to_string(base.join(GPU_PERFORMANCE_LEVEL_SUFFIX))
        .await
        .inspect_err(|message| error!("Error opening sysfs file for reading: {message}"))?;

    GPUPerformanceLevel::from_str(level.trim())
}

pub(crate) async fn set_gpu_performance_level(level: GPUPerformanceLevel) -> Result<()> {
    let level: String = level.to_string();
    let base = find_hwmon().await?;
    write_synced(base.join(GPU_PERFORMANCE_LEVEL_SUFFIX), level.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))
}

pub(crate) async fn set_gpu_clocks(clocks: u32) -> Result<()> {
    // Set GPU clocks to given value valid between 200 - 1600
    // Only used when GPU Performance Level is manual, but write whenever called.
    ensure!((200..=1600).contains(&clocks), "Invalid clocks");

    let base = find_hwmon().await?;
    let mut myfile = File::create(base.join(GPU_CLOCKS_SUFFIX))
        .await
        .inspect_err(|message| error!("Error opening sysfs file for writing: {message}"))?;

    let data = format!("s 0 {clocks}\n");
    myfile
        .write(data.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))?;
    myfile.flush().await?;

    let data = format!("s 1 {clocks}\n");
    myfile
        .write(data.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))?;
    myfile.flush().await?;

    myfile
        .write("c\n".as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))?;
    myfile.flush().await?;

    Ok(())
}

pub(crate) async fn get_gpu_clocks() -> Result<u32> {
    let base = find_hwmon().await?;
    let clocks_file = File::open(base.join(GPU_CLOCKS_SUFFIX)).await?;
    let mut reader = BufReader::new(clocks_file);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        if line != "OD_SCLK:\n" {
            continue;
        }

        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        let mhz = match line.split_whitespace().nth(1) {
            Some(mhz) if mhz.ends_with("Mhz") => mhz.trim_end_matches("Mhz"),
            _ => break,
        };

        return Ok(mhz.parse()?);
    }
    Ok(0)
}

async fn find_hwmon() -> Result<PathBuf> {
    let mut dir = fs::read_dir(path(GPU_HWMON_PREFIX)).await?;
    loop {
        let base = match dir.next_entry().await? {
            Some(entry) => entry.path(),
            None => bail!("hwmon not found"),
        };
        let file_name = base.join("name");
        let name = fs::read_to_string(file_name.as_path())
            .await?
            .trim()
            .to_string();
        if name == GPU_HWMON_NAME {
            return Ok(base);
        }
    }
}

pub(crate) async fn get_tdp_limit() -> Result<u32> {
    let base = find_hwmon().await?;
    let power1cap = fs::read_to_string(base.join(TDP_LIMIT1)).await?;
    let power1cap: u32 = power1cap.trim_end().parse()?;
    Ok(power1cap / 1000000)
}

pub(crate) async fn set_tdp_limit(limit: u32) -> Result<()> {
    // Set TDP limit given if within range (3-15)
    // Returns false on error or out of range
    ensure!((3..=15).contains(&limit), "Invalid limit");
    let data = format!("{limit}000000");

    let base = find_hwmon().await?;
    write_synced(base.join(TDP_LIMIT1), data.as_bytes())
        .await
        .inspect_err(|message| {
            error!("Error opening sysfs power1_cap file for writing TDP limits {message}")
        })?;

    if let Ok(mut power2file) = File::create(base.join(TDP_LIMIT2)).await {
        power2file
            .write(data.as_bytes())
            .await
            .inspect_err(|message| error!("Error writing to power2_cap file: {message}"))?;
        power2file.flush().await?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test {
    use super::*;
    use crate::{enum_roundtrip, testing};
    use anyhow::anyhow;
    use tokio::fs::{create_dir_all, read_to_string, remove_dir, write};

    pub async fn setup() {
        // Use hwmon5 just as a test. We needed a subfolder of GPU_HWMON_PREFIX
        // and this is as good as any.
        let base = path(GPU_HWMON_PREFIX).join("hwmon5");
        let filename = base.join(GPU_PERFORMANCE_LEVEL_SUFFIX);
        // Creates hwmon path, including device subpath
        create_dir_all(filename.parent().unwrap())
            .await
            .expect("create_dir_all");
        // Writes name file as addgpu so find_hwmon() will find it.
        write_synced(base.join("name"), GPU_HWMON_NAME.as_bytes())
            .await
            .expect("write_synced");
    }

    pub async fn write_clocks(mhz: u32) {
        let base = find_hwmon().await.unwrap();
        let filename = base.join(GPU_CLOCKS_SUFFIX);
        create_dir_all(filename.parent().unwrap())
            .await
            .expect("create_dir_all");

        let contents = format!(
            "OD_SCLK:
0:       {mhz}Mhz
1:       {mhz}Mhz
OD_RANGE:
SCLK:     200Mhz       1600Mhz
CCLK:    1400Mhz       3500Mhz
CCLK_RANGE in Core0:
0:       1400Mhz
1:       3500Mhz\n"
        );

        write(filename.as_path(), contents).await.expect("write");
    }

    pub async fn read_clocks() -> Result<String, std::io::Error> {
        let base = find_hwmon().await.unwrap();
        read_to_string(base.join(GPU_CLOCKS_SUFFIX))
            .await
    }

    pub fn format_clocks(mhz: u32) -> String {
        format!("s 0 {mhz}\ns 1 {mhz}\nc\n")
    }

    #[tokio::test]
    async fn test_get_gpu_performance_level() {
        let _h = testing::start();

        setup().await;
        let base = find_hwmon().await.unwrap();
        let filename = base.join(GPU_PERFORMANCE_LEVEL_SUFFIX);
        assert!(get_gpu_performance_level().await.is_err());

        write(filename.as_path(), "auto\n").await.expect("write");
        assert_eq!(
            get_gpu_performance_level().await.unwrap(),
            GPUPerformanceLevel::Auto
        );

        write(filename.as_path(), "low\n").await.expect("write");
        assert_eq!(
            get_gpu_performance_level().await.unwrap(),
            GPUPerformanceLevel::Low
        );

        write(filename.as_path(), "high\n").await.expect("write");
        assert_eq!(
            get_gpu_performance_level().await.unwrap(),
            GPUPerformanceLevel::High
        );

        write(filename.as_path(), "manual\n").await.expect("write");
        assert_eq!(
            get_gpu_performance_level().await.unwrap(),
            GPUPerformanceLevel::Manual
        );

        write(filename.as_path(), "peak_performance\n")
            .await
            .expect("write");
        assert_eq!(
            get_gpu_performance_level().await.unwrap(),
            GPUPerformanceLevel::ProfilePeak
        );

        write(filename.as_path(), "nothing\n").await.expect("write");
        assert!(get_gpu_performance_level().await.is_err());
    }

    #[tokio::test]
    async fn test_set_gpu_performance_level() {
        let _h = testing::start();

        setup().await;
        let base = find_hwmon().await.unwrap();
        let filename = base.join(GPU_PERFORMANCE_LEVEL_SUFFIX);

        set_gpu_performance_level(GPUPerformanceLevel::Auto)
            .await
            .expect("set");
        assert_eq!(
            read_to_string(filename.as_path()).await.unwrap().trim(),
            "auto"
        );
        set_gpu_performance_level(GPUPerformanceLevel::Low)
            .await
            .expect("set");
        assert_eq!(
            read_to_string(filename.as_path()).await.unwrap().trim(),
            "low"
        );
        set_gpu_performance_level(GPUPerformanceLevel::High)
            .await
            .expect("set");
        assert_eq!(
            read_to_string(filename.as_path()).await.unwrap().trim(),
            "high"
        );
        set_gpu_performance_level(GPUPerformanceLevel::Manual)
            .await
            .expect("set");
        assert_eq!(
            read_to_string(filename.as_path()).await.unwrap().trim(),
            "manual"
        );
        set_gpu_performance_level(GPUPerformanceLevel::ProfilePeak)
            .await
            .expect("set");
        assert_eq!(
            read_to_string(filename.as_path()).await.unwrap().trim(),
            "peak_performance"
        );
    }

    #[tokio::test]
    async fn test_get_tdp_limit() {
        let _h = testing::start();

        setup().await;
        let hwmon = path(GPU_HWMON_PREFIX);

        assert!(get_tdp_limit().await.is_err());

        write(hwmon.join("hwmon5").join(TDP_LIMIT1), "15000000\n")
            .await
            .expect("write");
        assert_eq!(get_tdp_limit().await.unwrap(), 15);
    }

    #[tokio::test]
    async fn test_set_tdp_limit() {
        let _h = testing::start();

        assert_eq!(
            set_tdp_limit(2).await.unwrap_err().to_string(),
            anyhow!("Invalid limit").to_string()
        );
        assert_eq!(
            set_tdp_limit(20).await.unwrap_err().to_string(),
            anyhow!("Invalid limit").to_string()
        );
        assert!(set_tdp_limit(10).await.is_err());

        let hwmon = path(GPU_HWMON_PREFIX);
        assert_eq!(
            set_tdp_limit(10).await.unwrap_err().to_string(),
            anyhow!("No such file or directory (os error 2)").to_string()
        );

        setup().await;
        let hwmon = hwmon.join("hwmon5");
        create_dir_all(hwmon.join(TDP_LIMIT1))
            .await
            .expect("create_dir_all");
        create_dir_all(hwmon.join(TDP_LIMIT2))
            .await
            .expect("create_dir_all");
        assert_eq!(
            set_tdp_limit(10).await.unwrap_err().to_string(),
            anyhow!("Is a directory (os error 21)").to_string()
        );

        remove_dir(hwmon.join(TDP_LIMIT1))
            .await
            .expect("remove_dir");
        write(hwmon.join(TDP_LIMIT1), "0").await.expect("write");
        assert!(set_tdp_limit(10).await.is_ok());
        let power1_cap = read_to_string(hwmon.join(TDP_LIMIT1))
            .await
            .expect("power1_cap");
        assert_eq!(power1_cap, "10000000");

        remove_dir(hwmon.join(TDP_LIMIT2))
            .await
            .expect("remove_dir");
        write(hwmon.join(TDP_LIMIT2), "0").await.expect("write");
        assert!(set_tdp_limit(15).await.is_ok());
        let power1_cap = read_to_string(hwmon.join(TDP_LIMIT1))
            .await
            .expect("power1_cap");
        assert_eq!(power1_cap, "15000000");
        let power2_cap = read_to_string(hwmon.join(TDP_LIMIT2))
            .await
            .expect("power2_cap");
        assert_eq!(power2_cap, "15000000");
    }

    #[tokio::test]
    async fn test_get_gpu_clocks() {
        let _h = testing::start();

        assert!(get_gpu_clocks().await.is_err());
        setup().await;

        let base = find_hwmon().await.unwrap();
        let filename = base.join(GPU_CLOCKS_SUFFIX);
        create_dir_all(filename.parent().unwrap())
            .await
            .expect("create_dir_all");
        write(filename.as_path(), b"").await.expect("write");

        assert_eq!(get_gpu_clocks().await.unwrap(), 0);
        write_clocks(1600).await;

        assert_eq!(get_gpu_clocks().await.unwrap(), 1600);
    }

    #[tokio::test]
    async fn test_set_gpu_clocks() {
        let _h = testing::start();

        assert!(set_gpu_clocks(1600).await.is_err());
        setup().await;

        assert!(set_gpu_clocks(100).await.is_err());
        assert!(set_gpu_clocks(2000).await.is_err());

        assert!(set_gpu_clocks(200).await.is_ok());

        assert_eq!(read_clocks().await.unwrap(), format_clocks(200));

        assert!(set_gpu_clocks(1600).await.is_ok());
        assert_eq!(read_clocks().await.unwrap(), format_clocks(1600));
    }

    #[test]
    fn gpu_performance_level_roundtrip() {
        enum_roundtrip!(GPUPerformanceLevel {
            0: u32 = Auto,
            1: u32 = Low,
            2: u32 = High,
            3: u32 = Manual,
            4: u32 = ProfilePeak,
            "auto": str = Auto,
            "low": str = Low,
            "high": str = High,
            "manual": str = Manual,
            "peak_performance": str = ProfilePeak,
        });
        assert!(GPUPerformanceLevel::try_from(5).is_err());
        assert!(GPUPerformanceLevel::from_str("profile_peak").is_err());
    }
}
