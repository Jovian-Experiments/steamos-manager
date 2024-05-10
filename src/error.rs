/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use zbus::fdo;

pub fn to_zbus_fdo_error<S: ToString>(error: S) -> fdo::Error {
    fdo::Error::Failed(error.to_string())
}

pub fn to_zbus_error<S: ToString>(error: S) -> zbus::Error {
    zbus::Error::Failure(error.to_string())
}

pub fn zbus_to_zbus_fdo(error: zbus::Error) -> fdo::Error {
    match error {
        zbus::Error::FDO(error) => *error,
        error => fdo::Error::Failed(error.to_string()),
    }
}
