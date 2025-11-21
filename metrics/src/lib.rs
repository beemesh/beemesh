use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn registry() -> &'static Mutex<HashMap<&'static str, f64>> {
    static REGISTRY: OnceLock<Mutex<HashMap<&'static str, f64>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn store_value(key: &'static str, value: f64, mode: StoreMode) {
    let mut guard = registry().lock().expect("metrics registry mutex poisoned");
    match mode {
        StoreMode::Counter => {
            let entry = guard.entry(key).or_insert(0.0);
            *entry += value;
        }
        StoreMode::Gauge => {
            guard.insert(key, value);
        }
    }
}

#[derive(Clone, Copy)]
enum StoreMode {
    Counter,
    Gauge,
}

#[doc(hidden)]
pub fn increment_counter_internal(name: &'static str, value: f64) {
    store_value(name, value, StoreMode::Counter);
}

#[doc(hidden)]
pub fn gauge_internal(name: &'static str, value: f64) {
    store_value(name, value, StoreMode::Gauge);
}

#[macro_export]
macro_rules! increment_counter {
    ($name:expr) => {
        $crate::increment_counter_internal($name, 1.0);
    };
    ($name:expr, $value:expr) => {
        $crate::increment_counter_internal($name, $value as f64);
    };
}

#[macro_export]
macro_rules! gauge {
    ($name:expr, $value:expr) => {
        $crate::gauge_internal($name, $value as f64);
    };
}
