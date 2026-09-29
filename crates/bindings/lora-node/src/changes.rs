//! Native half of `db.changes()`.
//!
//! The engine's [`ChangeFeed`] is pull-based. JS drives it with
//! `changesPoll()` (an `AsyncTask`, because resuming from an old LSN may
//! replay the WAL) and parks between polls until the feed's waker fires.
//! The waker is an unref'd threadsafe function: a waiting feed never keeps
//! the process alive on its own, and writers never block on JS.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{Env, Error as NapiError, JsObject, Status};

use lora_database::{
    Change, ChangeBatch, ChangeFeed, ChangeFeedCloser, ChangeFeedOptions, ChangePoll,
    Database as InnerDatabase, InMemoryGraph, LoraError, LoraValue,
};
use lora_store::Properties;

use crate::errors::format_lora_error;
use crate::to_napi::lora_value_to_napi;

pub(crate) type FeedRegistry = Arc<Mutex<FeedSet>>;

/// Feeds opened through one JS `Database` handle.
#[derive(Default)]
pub(crate) struct FeedSet {
    pub(crate) feeds: BTreeMap<u32, Arc<NativeFeed>>,
    /// Set by `dispose()`; feeds that finish opening afterwards are closed
    /// immediately.
    pub(crate) disposed: bool,
}

impl FeedSet {
    pub(crate) fn close_all(&mut self) {
        self.disposed = true;
        for feed in self.feeds.values() {
            feed.close();
        }
        self.feeds.clear();
    }
}

pub(crate) type Waker = ThreadsafeFunction<(), ErrorStrategy::Fatal>;

pub(crate) struct NativeFeed {
    feed: Mutex<Option<ChangeFeed>>,
    closer: ChangeFeedCloser,
    /// An error reached while a poll already had batches to return; it is
    /// reported by the next poll so no batch is lost.
    deferred: Mutex<Option<LoraError>>,
}

impl NativeFeed {
    pub(crate) fn close(&self) {
        self.closer.close();
    }
}

fn lock_err() -> NapiError {
    NapiError::new(Status::GenericFailure, "change feed registry poisoned")
}

pub(crate) fn lora_napi_error(err: &LoraError) -> NapiError {
    NapiError::new(Status::GenericFailure, format_lora_error(err))
}

pub struct OpenChangesTask {
    pub(crate) db: Arc<InnerDatabase<InMemoryGraph>>,
    pub(crate) options: ChangeFeedOptions,
    /// Opened synchronously on the JS thread when capture was already on
    /// (the common case), so commits after `changes()` returns are never
    /// missed. `None` means the worker must open it (first feed while a
    /// write holds the writer lock).
    pub(crate) opened: Option<std::result::Result<ChangeFeed, LoraError>>,
    pub(crate) waker: Option<Waker>,
    pub(crate) registry: FeedRegistry,
    pub(crate) id: u32,
}

impl Task for OpenChangesTask {
    type Output = ();
    type JsValue = u32;

    fn compute(&mut self) -> Result<Self::Output> {
        let feed = match self.opened.take() {
            Some(opened) => opened,
            None => self.db.changes(self.options),
        }
        .map_err(|e| lora_napi_error(&e))?;
        if let Some(waker) = self.waker.take() {
            feed.set_waker(move || {
                waker.call((), ThreadsafeFunctionCallMode::NonBlocking);
            });
        }
        let native = Arc::new(NativeFeed {
            closer: feed.closer(),
            feed: Mutex::new(Some(feed)),
            deferred: Mutex::new(None),
        });
        let mut registry = self.registry.lock().map_err(|_| lock_err())?;
        if registry.disposed {
            native.close();
            return Err(NapiError::new(
                Status::GenericFailure,
                crate::errors::closed_error_message(),
            ));
        }
        registry.feeds.insert(self.id, native);
        Ok(())
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> Result<Self::JsValue> {
        Ok(self.id)
    }
}

pub struct PollChangesOutput {
    batches: Vec<Arc<ChangeBatch>>,
    closed: bool,
}

pub struct PollChangesTask {
    pub(crate) feed: Arc<NativeFeed>,
    pub(crate) max: usize,
}

impl Task for PollChangesTask {
    type Output = PollChangesOutput;
    type JsValue = JsObject;

    fn compute(&mut self) -> Result<Self::Output> {
        if let Some(err) = self.feed.deferred.lock().map_err(|_| lock_err())?.take() {
            return Err(lora_napi_error(&err));
        }
        let mut slot = self.feed.feed.lock().map_err(|_| lock_err())?;
        let Some(feed) = slot.as_mut() else {
            return Ok(PollChangesOutput {
                batches: Vec::new(),
                closed: true,
            });
        };
        let mut batches = Vec::new();
        let mut closed = false;
        while batches.len() < self.max {
            match feed.poll() {
                Ok(ChangePoll::Batch(batch)) => batches.push(batch),
                Ok(ChangePoll::Pending) => break,
                Ok(ChangePoll::Closed) => {
                    closed = true;
                    break;
                }
                Err(err) => {
                    slot.take();
                    if batches.is_empty() {
                        return Err(lora_napi_error(&err));
                    }
                    *self.feed.deferred.lock().map_err(|_| lock_err())? = Some(err);
                    break;
                }
            }
        }
        if closed {
            slot.take();
        }
        Ok(PollChangesOutput { batches, closed })
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        let mut obj = env.create_object()?;
        let mut arr = env.create_array_with_length(output.batches.len())?;
        for (i, batch) in output.batches.iter().enumerate() {
            arr.set_element(i as u32, batch_to_napi(&env, batch)?)?;
        }
        obj.set_named_property("batches", arr)?;
        obj.set_named_property("closed", env.get_boolean(output.closed)?)?;
        Ok(obj)
    }
}

fn batch_to_napi(env: &Env, batch: &ChangeBatch) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    obj.set_named_property("lsn", env.create_double(batch.lsn as f64)?)?;
    let mut changes = env.create_array_with_length(batch.changes.len())?;
    for (i, change) in batch.changes.iter().enumerate() {
        changes.set_element(i as u32, change_to_napi(env, change)?)?;
    }
    obj.set_named_property("changes", changes)?;
    Ok(obj)
}

fn strings(env: &Env, values: &[String]) -> Result<JsObject> {
    let mut arr = env.create_array_with_length(values.len())?;
    for (i, value) in values.iter().enumerate() {
        arr.set_element(i as u32, env.create_string(value)?)?;
    }
    Ok(arr)
}

fn properties(env: &Env, props: &Properties) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    for (key, value) in props.iter() {
        obj.set_named_property(key, lora_value_to_napi(env, &LoraValue::from(value))?)?;
    }
    Ok(obj)
}

fn id(env: &Env, id: u64) -> Result<napi::JsNumber> {
    env.create_double(id as f64)
}

fn change_to_napi(env: &Env, change: &Change) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    let mut kind =
        |name: &str| -> Result<()> { obj.set_named_property("kind", env.create_string(name)?) };
    match change {
        Change::NodeCreated { .. } => kind("nodeCreated")?,
        Change::NodeUpdated { .. } => kind("nodeUpdated")?,
        Change::NodeDeleted { .. } => kind("nodeDeleted")?,
        Change::RelationshipCreated { .. } => kind("relationshipCreated")?,
        Change::RelationshipUpdated { .. } => kind("relationshipUpdated")?,
        Change::RelationshipDeleted { .. } => kind("relationshipDeleted")?,
        Change::Reset => kind("reset")?,
    }
    match change {
        Change::NodeCreated {
            id: node,
            labels,
            properties: props,
        }
        | Change::NodeDeleted {
            id: node,
            labels,
            properties: props,
        } => {
            obj.set_named_property("id", id(env, *node)?)?;
            obj.set_named_property("labels", strings(env, labels)?)?;
            obj.set_named_property("properties", properties(env, props)?)?;
        }
        Change::NodeUpdated {
            id: node,
            labels,
            properties: props,
            set_keys,
            removed_keys,
            added_labels,
            removed_labels,
        } => {
            obj.set_named_property("id", id(env, *node)?)?;
            obj.set_named_property("labels", strings(env, labels)?)?;
            obj.set_named_property("properties", properties(env, props)?)?;
            obj.set_named_property("setKeys", strings(env, set_keys)?)?;
            obj.set_named_property("removedKeys", strings(env, removed_keys)?)?;
            obj.set_named_property("addedLabels", strings(env, added_labels)?)?;
            obj.set_named_property("removedLabels", strings(env, removed_labels)?)?;
        }
        Change::RelationshipCreated {
            id: rel,
            rel_type,
            start,
            end,
            properties: props,
        }
        | Change::RelationshipDeleted {
            id: rel,
            rel_type,
            start,
            end,
            properties: props,
        } => {
            obj.set_named_property("id", id(env, *rel)?)?;
            obj.set_named_property("type", env.create_string(rel_type)?)?;
            obj.set_named_property("startId", id(env, *start)?)?;
            obj.set_named_property("endId", id(env, *end)?)?;
            obj.set_named_property("properties", properties(env, props)?)?;
        }
        Change::RelationshipUpdated {
            id: rel,
            rel_type,
            start,
            end,
            properties: props,
            set_keys,
            removed_keys,
        } => {
            obj.set_named_property("id", id(env, *rel)?)?;
            obj.set_named_property("type", env.create_string(rel_type)?)?;
            obj.set_named_property("startId", id(env, *start)?)?;
            obj.set_named_property("endId", id(env, *end)?)?;
            obj.set_named_property("properties", properties(env, props)?)?;
            obj.set_named_property("setKeys", strings(env, set_keys)?)?;
            obj.set_named_property("removedKeys", strings(env, removed_keys)?)?;
        }
        Change::Reset => {}
    }
    Ok(obj)
}
