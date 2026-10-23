//! Three-entry prepared-scene LRU, shared by retained CPU and GPU setup.
use pagura_core::{Error, Limits, Result, Store};
use pagura_interpret::Scene;
use pagura_paint::{PreparedScene, RenderRequest};

const MAX_ENTRIES: usize = 3;
const MAX_CHARGES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key(pub [u32; 7]);

pub struct Entry {
    pub key: Key,
    pub prepared: PreparedScene,
    pub id: u64,
}

#[derive(Default)]
pub struct Cache {
    entries: Vec<Entry>,
    charges: usize,
    next: u64,
    evictions: usize,
}

impl Cache {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.charges = 0;
    }

    pub fn ensure(
        &mut self,
        key: Key,
        scene: &Scene,
        request: &RenderRequest,
        resources: (&Store, &Limits),
    ) -> Result<bool> {
        if let Some(i) = self.entries.iter().position(|e| e.key == key) {
            let entry = self.entries.remove(i);
            self.entries.push(entry);
            return Ok(true);
        }
        let prepared = pagura_paint::prepare(scene, request, resources.0, resources.1)?;
        let charge = prepared.charges().render_bytes();
        if charge > MAX_CHARGES {
            drop(prepared);
            return Err(Error::limit("prepared cache entry exceeds budget"));
        }
        while self.entries.len() >= MAX_ENTRIES || self.charges > MAX_CHARGES - charge {
            let old = self.entries.remove(0);
            self.charges -= old.prepared.charges().render_bytes();
            drop(old.prepared);
            self.evictions += 1;
        }
        self.next += 1;
        self.charges += charge;
        self.entries.push(Entry {
            key,
            prepared,
            id: self.next,
        });
        Ok(false)
    }

    pub fn current(&self) -> Result<&Entry> {
        self.entries
            .last()
            .ok_or(Error::invalid_argument("empty prepared cache"))
    }

    pub fn metadata(&self) -> String {
        format!(
            "\"cache_entries\":{},\"cache_charged_bytes\":{},\"cache_evictions\":{}",
            self.entries.len(),
            self.charges,
            self.evictions
        )
    }
}
