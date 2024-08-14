/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 * Copyright © 2024 Igalia S.L.
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, bail, Error, Result};
use std::fmt;
use std::str::FromStr;
use zbus::Connection;

use crate::systemd::{daemon_reload, EnableState, SystemdUnit};

#[derive(PartialEq, Debug, Copy, Clone)]
pub enum HdmiCecState {
    Disabled = 0,
    ControlOnly = 1,
    ControlAndWake = 2,
}

impl TryFrom<u32> for HdmiCecState {
    type Error = Error;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == HdmiCecState::Disabled as u32 => Ok(HdmiCecState::Disabled),
            x if x == HdmiCecState::ControlOnly as u32 => Ok(HdmiCecState::ControlOnly),
            x if x == HdmiCecState::ControlAndWake as u32 => Ok(HdmiCecState::ControlAndWake),
            _ => Err(anyhow!("No enum match for value {v}")),
        }
    }
}

impl FromStr for HdmiCecState {
    type Err = Error;
    fn from_str(input: &str) -> Result<HdmiCecState, Self::Err> {
        Ok(match input.to_lowercase().as_str() {
            "disable" | "disabled" | "off" => HdmiCecState::Disabled,
            "control-only" | "controlonly" => HdmiCecState::ControlOnly,
            "control-wake" | "control-and-wake" | "controlandwake" => HdmiCecState::ControlAndWake,
            v => bail!("No enum match for value {v}"),
        })
    }
}

impl fmt::Display for HdmiCecState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            HdmiCecState::Disabled => write!(f, "Disabled"),
            HdmiCecState::ControlOnly => write!(f, "ControlOnly"),
            HdmiCecState::ControlAndWake => write!(f, "ControlAndWake"),
        }
    }
}

impl HdmiCecState {
    pub fn to_human_readable(&self) -> &'static str {
        match self {
            HdmiCecState::Disabled => "disabled",
            HdmiCecState::ControlOnly => "control-only",
            HdmiCecState::ControlAndWake => "control-and-wake",
        }
    }
}

pub(crate) struct HdmiCecControl<'dbus> {
    plasma_rc_unit: SystemdUnit<'dbus>,
    wakehook_unit: SystemdUnit<'dbus>,
    connection: Connection,
}

impl<'dbus> HdmiCecControl<'dbus> {
    pub async fn new(connection: &Connection) -> Result<HdmiCecControl<'dbus>> {
        Ok(HdmiCecControl {
            plasma_rc_unit: SystemdUnit::new(
                connection.clone(),
                "plasma-remotecontrollers.service",
            )
            .await?,
            wakehook_unit: SystemdUnit::new(connection.clone(), "wakehook.service").await?,
            connection: connection.clone(),
        })
    }

    pub async fn get_enabled_state(&self) -> Result<HdmiCecState> {
        Ok(match self.plasma_rc_unit.enabled().await? {
            EnableState::Enabled | EnableState::Static => {
                match self.wakehook_unit.enabled().await? {
                    EnableState::Enabled | EnableState::Static => HdmiCecState::ControlAndWake,
                    _ => HdmiCecState::ControlOnly,
                }
            }
            _ => HdmiCecState::Disabled,
        })
    }

    pub async fn set_enabled_state(&self, state: HdmiCecState) -> Result<()> {
        match state {
            HdmiCecState::Disabled => {
                self.plasma_rc_unit.mask().await?;
                self.plasma_rc_unit.stop().await?;
                self.wakehook_unit.mask().await?;
                self.wakehook_unit.stop().await?;
                daemon_reload(&self.connection).await?;
            }
            HdmiCecState::ControlOnly => {
                self.wakehook_unit.mask().await?;
                self.wakehook_unit.stop().await?;
                self.plasma_rc_unit.unmask().await?;
                daemon_reload(&self.connection).await?;
                self.plasma_rc_unit.start().await?;
            }
            HdmiCecState::ControlAndWake => {
                self.plasma_rc_unit.unmask().await?;
                self.wakehook_unit.unmask().await?;
                daemon_reload(&self.connection).await?;
                self.plasma_rc_unit.start().await?;
                self.wakehook_unit.start().await?;
            }
        };

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::enum_roundtrip;

    #[test]
    fn hdmi_cec_state_roundtrip() {
        enum_roundtrip!(HdmiCecState {
            0: u32 = Disabled,
            1: u32 = ControlOnly,
            2: u32 = ControlAndWake,
            "Disabled": str = Disabled,
            "ControlOnly": str = ControlOnly,
            "ControlAndWake": str = ControlAndWake,
        });
        assert_eq!(
            HdmiCecState::from_str("control-only").unwrap(),
            HdmiCecState::ControlOnly
        );
        assert_eq!(
            HdmiCecState::from_str("control-and-wake").unwrap(),
            HdmiCecState::ControlAndWake
        );
        assert_eq!(HdmiCecState::Disabled.to_human_readable(), "disabled");
        assert_eq!(
            HdmiCecState::ControlOnly.to_human_readable(),
            "control-only"
        );
        assert_eq!(
            HdmiCecState::ControlAndWake.to_human_readable(),
            "control-and-wake"
        );
        assert!(HdmiCecState::try_from(3).is_err());
        assert!(HdmiCecState::from_str("working").is_err());
    }
}
