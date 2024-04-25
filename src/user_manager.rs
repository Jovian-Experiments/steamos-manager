/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 * Copyright © 2024 Igalia S.L.
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use zbus::{interface, Connection};

use crate::API_VERSION;

pub struct SteamOSManagerUser {
    connection: Connection,
}

impl SteamOSManagerUser {
    pub async fn new(connection: Connection) -> Result<Self> {
        Ok(SteamOSManagerUser { connection })
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.UserManager")]
impl SteamOSManagerUser {
    #[zbus(property(emits_changed_signal = "const"))]
    async fn version(&self) -> u32 {
        API_VERSION
    }
}
