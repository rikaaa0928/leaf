use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct RoutingRecord {
    pub network: String,
    pub source: String,
    pub destination: String,
    pub inbound_tag: String,
    pub outbound_tag: String,
    pub timestamp: u64,
}

pub struct RoutingHistory {
    max_records: AtomicUsize,
    enabled: AtomicBool,
    records: Mutex<VecDeque<RoutingRecord>>,
}

impl RoutingHistory {
    pub fn new() -> Self {
        Self {
            max_records: AtomicUsize::new(0),
            enabled: AtomicBool::new(false),
            records: Mutex::new(VecDeque::new()),
        }
    }

    pub fn set_enabled(&self, enabled: bool, max_records: usize) {
        let mut records = self.records.lock().unwrap();
        self.enabled.store(enabled, Ordering::SeqCst);
        self.max_records.store(max_records, Ordering::SeqCst);
        if !enabled {
            records.clear();
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn add(&self, record: RoutingRecord) {
        let mut records = self.records.lock().unwrap();
        if !self.is_enabled() {
            return;
        }
        records.push_back(record);
        let max = self.max_records.load(Ordering::SeqCst);
        let max = if max == 0 { 1 } else { max };
        while records.len() > max {
            records.pop_front();
        }
    }

    pub fn get_records(&self) -> Vec<RoutingRecord> {
        let records = self.records.lock().unwrap();
        records.iter().cloned().collect()
    }
}

impl Default for RoutingHistory {
    fn default() -> Self {
        Self::new()
    }
}
