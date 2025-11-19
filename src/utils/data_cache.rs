use std::{
    collections::{HashMap, hash_map::Entry},
    time::Instant,
};

#[derive(Debug)]
pub struct CacheEntry<T> {
    pub payload: T,
    pub refcount: usize,
    pub unload_at: Option<Instant>,
}

impl<T> CacheEntry<T> {
    /// Creates a cache entry with an initial reference count of one.
    pub fn new(payload: T) -> Self {
        Self {
            payload,
            refcount: 1,
            unload_at: None,
        }
    }

    /// Marks the cache entry for unloading at the specified time.
    pub fn mark_for_unload(&mut self, when: Instant) {
        self.unload_at = Some(when);
    }

    /// Cancels any pending unload for the entry.
    pub fn clear_unload(&mut self) {
        self.unload_at = None;
    }
}

#[derive(Debug, Default)]
pub struct DataCache<T> {
    data: HashMap<String, CacheEntry<T>>,
}

impl<T> DataCache<T> {
    /// Creates an empty cache.
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    /// Returns an immutable reference to a cached entry by key.
    pub fn get(&self, key: &str) -> Option<&CacheEntry<T>> {
        self.data.get(key)
    }

    /// Returns a mutable reference to a cached entry by key.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut CacheEntry<T>> {
        self.data.get_mut(key)
    }

    /// Inserts a new entry or increments the reference count on an existing one.
    pub fn insert_or_increment<F>(&mut self, key: &str, create: F) -> &mut CacheEntry<T>
    where
        F: FnOnce() -> T,
    {
        match self.data.entry(key.to_string()) {
            Entry::Occupied(mut occupied) => {
                {
                    let entry = occupied.get_mut();
                    entry.refcount += 1;
                    entry.clear_unload();
                }
                occupied.into_mut()
            }
            Entry::Vacant(vacant) => vacant.insert(CacheEntry::new(create())),
        }
    }

    /// Decrements the reference count for a key and schedules unload when it reaches zero.
    pub fn decrement(&mut self, key: &str, unload_at: Instant) -> Option<&mut CacheEntry<T>> {
        if let Some(entry) = self.data.get_mut(key) {
            if entry.refcount > 0 {
                entry.refcount -= 1;
            }

            if entry.refcount == 0 {
                entry.mark_for_unload(unload_at);
            }

            Some(entry)
        } else {
            None
        }
    }

    /// Removes and returns any entries whose unload time has expired.
    pub fn drain_expired(&mut self, now: Instant) -> Vec<(String, CacheEntry<T>)> {
        let expired_keys: Vec<String> = self
            .data
            .iter()
            .filter_map(|(key, entry)| match entry.unload_at {
                Some(when) if when <= now => Some(key.clone()),
                _ => None,
            })
            .collect();

        expired_keys
            .into_iter()
            .filter_map(|key| self.data.remove(&key).map(|entry| (key, entry)))
            .collect()
    }
}
