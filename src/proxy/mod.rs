/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

// Re-export relevant proxies

// TODO Some of these should get renamed
mod job;
mod job_manager;
mod udev_events;
pub use crate::proxy::job::JobProxy;
pub use crate::proxy::job_manager::JobManagerProxy;
pub use crate::proxy::udev_events::UdevEventsProxy;

// Deprecated interface
mod manager;
pub use crate::proxy::manager::ManagerProxy;

// Optional interfaces
mod ambient_light_sensor1;
mod cpu_scaling1;
mod factory_reset1;
mod fan_control1;
mod gpu_performance_level1;
mod gpu_power_profile1;
mod gpu_tdp_limit1;
mod hdmi_cec1;
mod manager2;
mod storage1;
mod update_bios1;
mod update_dock1;
mod wifi_debug1;
mod wifi_power_management1;
pub use crate::proxy::ambient_light_sensor1::AmbientLightSensor1Proxy;
pub use crate::proxy::cpu_scaling1::CpuScaling1Proxy;
pub use crate::proxy::factory_reset1::FactoryReset1Proxy;
pub use crate::proxy::fan_control1::FanControl1Proxy;
pub use crate::proxy::gpu_performance_level1::GpuPerformanceLevel1Proxy;
pub use crate::proxy::gpu_power_profile1::GpuPowerProfile1Proxy;
pub use crate::proxy::gpu_tdp_limit1::GpuTdpLimit1Proxy;
pub use crate::proxy::hdmi_cec1::HdmiCec1Proxy;
pub use crate::proxy::manager2::Manager2Proxy;
pub use crate::proxy::storage1::Storage1Proxy;
pub use crate::proxy::update_bios1::UpdateBios1Proxy;
pub use crate::proxy::update_dock1::UpdateDock1Proxy;
pub use crate::proxy::wifi_debug1::WifiDebug1Proxy;
pub use crate::proxy::wifi_power_management1::WifiPowerManagement1Proxy;
