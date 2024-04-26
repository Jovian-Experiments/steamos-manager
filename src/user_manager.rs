/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 * Copyright © 2024 Igalia S.L.
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use tracing::error;
use zbus::{interface, Connection};

use crate::{to_zbus_error, to_zbus_fdo_error, API_VERSION};
use crate::cec::{HdmiCecControl, HdmiCecState};

pub struct SteamOSManagerUser {
    connection: Connection,
    hdmi_cec: HdmiCecControl<'static>,
}

impl SteamOSManagerUser {
    pub async fn new(connection: Connection) -> Result<Self> {
        Ok(SteamOSManagerUser {
            hdmi_cec: HdmiCecControl::new(&connection).await?,
            connection,
        })
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.UserManager")]
impl SteamOSManagerUser {
    #[zbus(property(emits_changed_signal = "false"))]
    async fn hdmi_cec_state(&self) -> zbus::fdo::Result<u32> {
        match self.hdmi_cec.get_enabled_state().await {
            Ok(state) => Ok(state as u32),
            Err(e) => Err(to_zbus_fdo_error(e)),
        }
    }

    #[zbus(property)]
    async fn set_hdmi_cec_state(&self, state: u32) -> zbus::Result<()> {
        let state = match HdmiCecState::try_from(state) {
            Ok(state) => state,
            Err(err) => return Err(zbus::fdo::Error::InvalidArgs(err.to_string()).into()),
        };
        self.hdmi_cec
            .set_enabled_state(state)
            .await
            .inspect_err(|message| error!("Error setting CEC state: {message}"))
            .map_err(to_zbus_error)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    async fn version(&self) -> u32 {
        API_VERSION
    }
}
