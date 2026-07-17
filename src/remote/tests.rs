#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::*;
    use crate::dot_rev::DotRev;
    use tempfile::tempdir;
    
    #[test]
    fn test_remote_config_management() {
        let temp_dir = tempdir().unwrap();
        let rev_path = temp_dir.path().join(".rev");
        
        // Initialize repository
        let dot_rev = DotRev::init(rev_path).unwrap();
        
        // Initially no remotes
        let remotes = dot_rev.remotes().unwrap();
        assert_eq!(remotes.len(), 0);
        
        // Add a remote
        let remote1 = RemoteConfig {
            name: "origin".to_string(),
            url: "http://example.com:8080".to_string(),
        };
        dot_rev.add_remote(remote1.clone()).unwrap();
        
        // Check it was added
        let remotes = dot_rev.remotes().unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].url, "http://example.com:8080");
        
        // Get remote by name
        let origin = dot_rev.get_remote("origin").unwrap();
        assert!(origin.is_some());
        assert_eq!(origin.unwrap().url, "http://example.com:8080");
        
        // Add another remote
        let remote2 = RemoteConfig {
            name: "backup".to_string(),
            url: "http://backup.example.com:8080".to_string(),
        };
        dot_rev.add_remote(remote2).unwrap();
        
        // Check both exist
        let remotes = dot_rev.remotes().unwrap();
        assert_eq!(remotes.len(), 2);
        
        // Remove a remote
        dot_rev.remove_remote("origin").unwrap();
        
        // Check it was removed
        let remotes = dot_rev.remotes().unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "backup");
        
        // Try to add duplicate
        let duplicate = RemoteConfig {
            name: "backup".to_string(),
            url: "http://another.example.com:8080".to_string(),
        };
        assert!(dot_rev.add_remote(duplicate).is_err());
    }
    
    mod sync_tests {
        use super::super::super::*;
        use crate::directory::{Directory, DirectoryEntry};
        use crate::dot_rev::InsertJson;
        use crate::object_id::ObjectId;
        use crate::object_store::{in_memory::InMemoryObjectStore, ObjectStore};
        use crate::snapshot::SnapShot;
        use std::cell::RefCell;
        use std::collections::{BTreeMap, HashMap, HashSet};

        #[derive(Debug)]
        struct MockError(String);
        impl std::fmt::Display for MockError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl std::error::Error for MockError {}

        /// An in-memory RemoteRepository that records how it is called, so
        /// tests can assert on fetch counts and upload batching shape.
        #[derive(Default)]
        struct MockRemote {
            objects: RefCell<HashMap<ObjectId, Vec<u8>>>,
            branches: RefCell<HashMap<String, ObjectId>>,
            /// (count, raw bytes) of every UploadObjects batch received.
            upload_batches: RefCell<Vec<(usize, usize)>>,
            /// (offset, len) of every chunk received, per object.
            chunks: RefCell<HashMap<ObjectId, Vec<(u64, usize)>>>,
            chunk_staging: RefCell<HashMap<ObjectId, Vec<u8>>>,
            /// Every id requested via get_objects, in request order.
            fetched: RefCell<Vec<ObjectId>>,
            get_object_calls: RefCell<usize>,
            /// Ids to answer with None even though present.
            withhold: RefCell<HashSet<ObjectId>>,
        }

        impl RemoteRepository for MockRemote {
            type Error = MockError;

            fn get_info(&self) -> Result<(String, Vec<String>), MockError> {
                Ok(("mock".to_string(), self.branches.borrow().keys().cloned().collect()))
            }
            fn list_branches(&self) -> Result<Vec<String>, MockError> {
                Ok(self.branches.borrow().keys().cloned().collect())
            }
            fn get_branch_snapshot(&self, branch: &str) -> Result<Option<ObjectId>, MockError> {
                Ok(self.branches.borrow().get(branch).copied())
            }
            fn has_object(&self, id: ObjectId) -> Result<bool, MockError> {
                Ok(self.objects.borrow().contains_key(&id))
            }
            fn get_object(&self, id: ObjectId) -> Result<Option<Vec<u8>>, MockError> {
                *self.get_object_calls.borrow_mut() += 1;
                Ok(self.objects.borrow().get(&id).cloned())
            }
            fn get_objects(&self, ids: &[ObjectId]) -> Result<Vec<FetchedObject>, MockError> {
                self.fetched.borrow_mut().extend_from_slice(ids);
                Ok(ids
                    .iter()
                    .map(|id| {
                        let data = if self.withhold.borrow().contains(id) {
                            None
                        } else {
                            self.objects.borrow().get(id).cloned()
                        };
                        (*id, data)
                    })
                    .collect())
            }
            fn push_snapshot(
                &self,
                branch: &str,
                snapshot_id: ObjectId,
                _force: bool,
            ) -> Result<ObjectId, MockError> {
                self.branches.borrow_mut().insert(branch.to_string(), snapshot_id);
                Ok(snapshot_id)
            }
            fn upload_object(&self, id: ObjectId, data: &[u8]) -> Result<(), MockError> {
                self.objects.borrow_mut().insert(id, data.to_vec());
                Ok(())
            }
            fn upload_objects(&self, objects: &[(ObjectId, Vec<u8>)]) -> Result<(), MockError> {
                let bytes = objects.iter().map(|(_, d)| d.len()).sum();
                self.upload_batches.borrow_mut().push((objects.len(), bytes));
                for (id, data) in objects {
                    self.objects.borrow_mut().insert(*id, data.clone());
                }
                Ok(())
            }
            fn upload_object_chunk(
                &self,
                id: ObjectId,
                offset: u64,
                total_size: u64,
                data: &[u8],
            ) -> Result<(), MockError> {
                self.chunks.borrow_mut().entry(id).or_default().push((offset, data.len()));
                let mut staging = self.chunk_staging.borrow_mut();
                let buf = staging.entry(id).or_default();
                if offset as usize != buf.len() {
                    return Err(MockError(format!(
                        "out of order chunk: offset {offset}, have {}",
                        buf.len()
                    )));
                }
                buf.extend_from_slice(data);
                if buf.len() as u64 == total_size {
                    let bytes = staging.remove(&id).unwrap();
                    self.objects.borrow_mut().insert(id, bytes);
                }
                Ok(())
            }
        }

        /// Builds a snapshot in `store` from (path, content) pairs (flat paths
        /// only) with the given parents, returning its id.
        fn make_snapshot(
            store: &mut InMemoryObjectStore,
            parents: Vec<ObjectId>,
            files: &[(&str, &[u8])],
        ) -> ObjectId {
            let mut root = BTreeMap::new();
            for (name, content) in files {
                let id = store.insert(content).unwrap();
                root.insert(name.to_string(), DirectoryEntry::File(id));
            }
            let dir_id = store.insert_json(&Directory { root }).unwrap();
            store
                .insert_json(&SnapShot {
                    message: "test".to_string(),
                    directory: dir_id,
                    previous: parents,
                })
                .unwrap()
        }

        #[test]
        fn push_then_fresh_pull_fetches_each_object_exactly_once() {
            let mut local = InMemoryObjectStore::new();
            let s1 = make_snapshot(&mut local, vec![], &[("a.txt", b"aaa"), ("b.txt", b"bbb")]);
            let s2 = make_snapshot(&mut local, vec![s1], &[("a.txt", b"aaa2"), ("b.txt", b"bbb")]);

            let remote = MockRemote::default();
            sync::push_branch(&remote, &mut local, "dev", s2, false).unwrap();
            assert_eq!(remote.get_branch_snapshot("dev").unwrap(), Some(s2));

            // Fresh clone: every object must arrive, each fetched exactly once,
            // and only via the batch API.
            let mut clone = InMemoryObjectStore::new();
            let tip = sync::pull_branch(&remote, &mut clone, "dev").unwrap();
            assert_eq!(tip, Some(s2));

            let fetched = remote.fetched.borrow();
            let unique: HashSet<_> = fetched.iter().collect();
            assert_eq!(
                fetched.len(),
                unique.len(),
                "an object was requested more than once: {fetched:?}"
            );
            assert_eq!(*remote.get_object_calls.borrow(), 0, "single-object fetches used");

            // The pulled graph is complete and byte-identical.
            for (id, data) in remote.objects.borrow().iter() {
                assert_eq!(clone.read(*id).unwrap().as_deref(), Some(data.as_slice()));
            }
        }

        #[test]
        fn pull_missing_object_is_a_hard_error() {
            let mut local = InMemoryObjectStore::new();
            let s1 = make_snapshot(&mut local, vec![], &[("a.txt", b"content-a")]);
            let remote = MockRemote::default();
            sync::push_branch(&remote, &mut local, "dev", s1, false).unwrap();

            // Withhold one blob: the server claims the branch but can't
            // produce the object. The pull must fail, not "succeed" with a
            // hole in the graph.
            let blob_id = ObjectId::from(&b"content-a"[..]);
            remote.withhold.borrow_mut().insert(blob_id);

            let mut clone = InMemoryObjectStore::new();
            let err = sync::pull_branch(&remote, &mut clone, "dev");
            assert!(
                matches!(err, Err(sync::SyncError::ObjectMissing(id)) if id == blob_id),
                "expected ObjectMissing({blob_id}), got {err:?}"
            );
        }

        #[test]
        fn snapshot_shaped_file_content_does_not_break_push() {
            // Regression: the traversal used to guess object types by parsing,
            // so a checked-in file whose bytes are valid snapshot JSON injected
            // a phantom directory id into the graph and push failed with
            // "object missing" for an id that never existed.
            let mut local = InMemoryObjectStore::new();
            let phantom = SnapShot {
                message: "i am file content, not a snapshot".to_string(),
                directory: ObjectId::from(&b"this object does not exist"[..]),
                previous: vec![],
            };
            let trap_bytes = serde_json::to_vec_pretty(&phantom).unwrap();
            // Sanity: the trap really does parse as a snapshot.
            assert!(serde_json::from_slice::<SnapShot>(&trap_bytes).is_ok());

            let trap_ref: &[u8] = &trap_bytes;
            let s1 = make_snapshot(&mut local, vec![], &[("trap.json", trap_ref)]);

            let remote = MockRemote::default();
            sync::push_branch(&remote, &mut local, "dev", s1, false)
                .expect("push must not chase ids inside file content");

            // And the round trip preserves the file byte-for-byte.
            let mut clone = InMemoryObjectStore::new();
            sync::pull_branch(&remote, &mut clone, "dev").unwrap();
            let blob_id = ObjectId::from(trap_ref);
            assert_eq!(clone.read(blob_id).unwrap().as_deref(), Some(trap_ref));
        }

        #[test]
        fn upload_batches_respect_byte_and_count_limits() {
            let remote = MockRemote::default();
            let limits = sync::UploadLimits { batch_bytes: 100, batch_count: 3, chunk_bytes: 40 };

            // Distinct contents so ids are distinct.
            let mk = |byte: u8, len: usize| vec![byte; len];
            let objects: Vec<(ObjectId, Vec<u8>)> = [
                mk(1, 40), mk(2, 40), mk(3, 30), // 40+40 flushes before 30 (110 > 100)
                mk(4, 10), mk(5, 10),            // count cap: 30+10+10 = 3 objects
                mk(6, 150),                       // oversized: chunked path
                mk(7, 5),
            ]
            .into_iter()
            .map(|d| (ObjectId::from(d.as_slice()), d))
            .collect();

            sync::upload_in_batches(&remote, &objects, &limits).unwrap();

            for (count, bytes) in remote.upload_batches.borrow().iter() {
                assert!(*count <= 3, "batch count {count} exceeds limit");
                assert!(*bytes <= 100, "batch bytes {bytes} exceed limit");
            }

            // The oversized object went through in-order chunks and reassembled.
            let big_id = ObjectId::from(&vec![6u8; 150][..]);
            let chunks = remote.chunks.borrow();
            assert_eq!(chunks.get(&big_id).unwrap(), &vec![(0, 40), (40, 40), (80, 40), (120, 30)]);
            assert_eq!(remote.objects.borrow().get(&big_id).unwrap(), &vec![6u8; 150]);

            // Every object arrived.
            for (id, data) in &objects {
                assert_eq!(remote.objects.borrow().get(id).unwrap(), data);
            }
        }
    }

    #[test]
    fn test_remote_request_response_serialization() {
        use serde_json;
        
        // Test GetInfo request
        let request = RemoteRequest::GetInfo;
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: RemoteRequest = serde_json::from_str(&json).unwrap();
        match deserialized {
            RemoteRequest::GetInfo => {},
            _ => panic!("Wrong request type"),
        }
        
        // Test Info response
        let response = RemoteResponse::Info {
            name: "test-repo".to_string(),
            branches: vec!["main".to_string(), "dev".to_string()],
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: RemoteResponse = serde_json::from_str(&json).unwrap();
        match deserialized {
            RemoteResponse::Info { name, branches } => {
                assert_eq!(name, "test-repo");
                assert_eq!(branches.len(), 2);
            },
            _ => panic!("Wrong response type"),
        }
    }
}