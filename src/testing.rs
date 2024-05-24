use anyhow::{anyhow, Result};
use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::path::Path;
use std::rc::Rc;
use tempfile::{tempdir, TempDir};

thread_local! {
    static TEST: RefCell<Option<Rc<Test>>> = RefCell::new(None);
}

#[macro_export]
macro_rules! enum_roundtrip {
    ($enum:ident => $value:literal : str = $variant:ident) => {
        assert_eq!($enum::$variant.to_string(), $value);
        assert_eq!($enum::from_str($value).unwrap(), $enum::$variant);
    };
    ($enum:ident => $value:literal : $ty:ty = $variant:ident) => {
        assert_eq!($enum::$variant as $ty, $value);
        assert_eq!($enum::try_from($value), Ok($enum::$variant));
    };

    ($enum:ident { $($value:literal : $ty:ident = $variant:ident,)+ }) => {
        $(enum_roundtrip!($enum => $value : $ty = $variant);)+
    };
}

#[macro_export]
macro_rules! enum_on_off {
    ($enum:ident => ($on:ident, $off:ident)) => {
        assert_eq!($enum::from_str("on").unwrap(), $enum::$on);
        assert_eq!($enum::from_str("On").unwrap(), $enum::$on);
        assert_eq!($enum::from_str("enable").unwrap(), $enum::$on);
        assert_eq!($enum::from_str("enabled").unwrap(), $enum::$on);
        assert_eq!($enum::from_str("1").unwrap(), $enum::$on);
        assert_eq!($enum::from_str("off").unwrap(), $enum::$off);
        assert_eq!($enum::from_str("Off").unwrap(), $enum::$off);
        assert_eq!($enum::from_str("disable").unwrap(), $enum::$off);
        assert_eq!($enum::from_str("disabled").unwrap(), $enum::$off);
        assert_eq!($enum::from_str("0").unwrap(), $enum::$off);
    }
}

pub fn start() -> TestHandle {
    TEST.with(|lock| {
        assert!(lock.borrow().as_ref().is_none());
        let test: Rc<Test> = Rc::new(Test {
            base: tempdir().expect("Couldn't create test directory"),
            process_cb: Cell::new(|_, _| Err(anyhow!("No current process_cb"))),
        });
        *lock.borrow_mut() = Some(test.clone());
        TestHandle { test }
    })
}

pub fn stop() {
    TEST.with(|lock| *lock.borrow_mut() = None);
}

pub fn current() -> Rc<Test> {
    TEST.with(|lock| lock.borrow().as_ref().unwrap().clone())
}

pub struct Test {
    base: TempDir,
    pub process_cb: Cell<fn(&str, &[&OsStr]) -> Result<(i32, String)>>,
}

pub struct TestHandle {
    pub test: Rc<Test>,
}

impl Test {
    pub fn path(&self) -> &Path {
        self.base.path()
    }
}

impl Drop for TestHandle {
    fn drop(&mut self) {
        stop();
    }
}
