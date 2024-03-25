/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use tokio::fs;

use crate::path;

const BOARD_VENDOR_PATH: &str = "/sys/class/dmi/id/board_vendor";
const BOARD_NAME_PATH: &str = "/sys/class/dmi/id/board_name";
const VALVE_VENDOR: &str = "Valve";
const JUPITER_NAME: &str = "Jupiter";
const GALILEO_NAME: &str = "Galileo";

#[derive(PartialEq, Debug, Copy, Clone)]
pub enum HardwareVariant {
    Unknown,
    Jupiter,
    Galileo,
}

pub async fn variant() -> Result<HardwareVariant> {
    let board_vendor = fs::read_to_string(path(BOARD_VENDOR_PATH)).await?;
    if board_vendor.trim_end() != VALVE_VENDOR {
        return Ok(HardwareVariant::Unknown);
    }

    let board_name = fs::read_to_string(path(BOARD_NAME_PATH)).await?;
    Ok(match board_name.trim_end() {
        JUPITER_NAME => HardwareVariant::Jupiter,
        GALILEO_NAME => HardwareVariant::Galileo,
        _ => HardwareVariant::Unknown,
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing;
    use tokio::fs::{create_dir_all, write};

    #[tokio::test]
    async fn board_lookup() {
        let h = testing::start();

        create_dir_all(crate::path("/sys/class/dmi/id"))
            .await
            .expect("create_dir_all");
        assert!(variant().await.is_err());

        write(crate::path("/sys/class/dmi/id/board_vendor"), "LENOVO\n")
            .await
            .expect("write");
        assert_eq!(variant().await.unwrap(), HardwareVariant::Unknown);

        write(crate::path("/sys/class/dmi/id/board_vendor"), "Valve\n")
            .await
            .expect("write");
        write(crate::path("/sys/class/dmi/id/board_name"), "Jupiter\n")
            .await
            .expect("write");
        assert_eq!(variant().await.unwrap(), HardwareVariant::Jupiter);

        write(crate::path("/sys/class/dmi/id/board_name"), "Galileo\n")
            .await
            .expect("write");
        assert_eq!(variant().await.unwrap(), HardwareVariant::Galileo);

        write(crate::path("/sys/class/dmi/id/board_name"), "Neptune\n")
            .await
            .expect("write");
        assert_eq!(variant().await.unwrap(), HardwareVariant::Unknown);
    }
}
