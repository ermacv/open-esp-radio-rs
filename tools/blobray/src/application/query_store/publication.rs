//! Output manifest publication using immutable queries and existing CAS roots.

use super::*;
use crate::application::{
    output_set::OutputReceipt,
    published_outputs::{PublishedOutput, PublishedOutputManifest, manifest_key, output_key},
};

impl QueryStore {
    /// Retain every completed output, including outputs of uncached work, and
    /// activate their manifest with the run's existing epoch transaction.
    pub(in crate::application) fn publish_analysis_outputs(
        &mut self,
        receipts: &[OutputReceipt],
    ) -> Result<()> {
        self.publish_analysis_outputs_observed(receipts, || {})
    }

    fn publish_analysis_outputs_observed(
        &mut self,
        receipts: &[OutputReceipt],
        before_activation: impl FnOnce(),
    ) -> Result<()> {
        self.validate_root_identity()?;
        let epoch = self.publishing_epoch.clone().ok_or_else(|| {
            crate::Error::invalid("output publication requires a project-analysis epoch")
        })?;
        let mut outputs = receipts
            .iter()
            .map(|receipt| PublishedOutput {
                path: receipt.path().to_owned(),
                bytes: receipt.bytes(),
                sha256: receipt.sha256().to_owned(),
            })
            .collect::<Vec<_>>();
        outputs.sort_by(|a, b| a.path.cmp(&b.path));
        let manifest = PublishedOutputManifest {
            schema: 2,
            epoch: epoch.clone(),
            project_manifest: self.project_manifest.clone(),
            outputs,
        };
        manifest.validate(&epoch)?;
        receipts.iter().try_for_each(OutputReceipt::validate)?;
        self.ensure_file_objects(
            &receipts
                .iter()
                .map(|receipt| {
                    (
                        String::new(),
                        receipt.sha256().to_owned(),
                        receipt.path().to_owned(),
                    )
                })
                .collect::<Vec<_>>(),
        )?;

        // Direct immutable result locations keep output payloads streaming at
        // ingestion. Their ordinary query roots/dependencies protect them in
        // retention and compaction without introducing another GC graph.
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| store_error("begin output query publication", error))?;
        let mut dependencies = BTreeSet::new();
        for output in &manifest.outputs {
            let key = output_key(&output.sha256);
            transaction.execute(
                "INSERT INTO query_results(query_key, kind, input_fingerprint, result_digest, inline_value, object_digest)
                 VALUES (?1, 'analysis-output', ?2, ?2, NULL, ?2) ON CONFLICT(query_key) DO NOTHING",
                params![&key, &output.sha256],
            ).map_err(|error| store_error("retain published output content", error))?;
            let valid: bool = transaction.query_row(
                "SELECT kind = 'analysis-output' AND input_fingerprint = ?2 AND result_digest = ?2
                    AND inline_value IS NULL AND object_digest = ?2
                    AND NOT EXISTS (SELECT 1 FROM query_dependencies WHERE query_key = ?1)
                 FROM query_results WHERE query_key = ?1",
                params![&key, &output.sha256], |row| row.get(0),
            ).map_err(|error| store_error("validate immutable output query", error))?;
            if !valid {
                return Err(crate::Error::invalid(
                    "published output query identity collision",
                ));
            }
            attach_query_to_epoch(&transaction, &epoch, &key)?;
            transaction
                .execute(
                    "DELETE FROM retired_objects WHERE digest = ?1",
                    [&output.sha256],
                )
                .map_err(|error| store_error("retain published output object", error))?;
            dependencies.insert(key);
        }
        transaction
            .commit()
            .map_err(|error| store_error("commit output query publication", error))?;
        self.put(
            &manifest_key(&epoch),
            "analysis-output-manifest",
            &epoch,
            &dependencies.into_iter().collect::<Vec<_>>(),
            &serde_json::to_vec(&manifest)?,
        )?;
        before_activation();
        receipts.iter().try_for_each(OutputReceipt::validate)?;
        self.complete_analysis_epoch()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{PublishedAnalysisOutputs, output_set::OutputSet};

    fn emit(path: &Path, bytes: &[u8]) -> Vec<OutputReceipt> {
        let set = OutputSet::new(&[path.to_owned()], false).unwrap();
        set.file(0, "fixture").unwrap().bytes(bytes).unwrap();
        set.receipts().unwrap()
    }

    #[test]
    fn published_outputs_survive_file_deletion_next_generation_and_compaction() {
        let manifest = super::super::tests::manifest("output-manifest-history");
        assert!(PublishedAnalysisOutputs::open(&manifest).unwrap().is_none());
        assert!(!manifest.parent().unwrap().join("generated").exists());
        let path = manifest.parent().unwrap().join("generated/result.json");
        let alias = path.with_file_name("same-content.json");
        let empty = path.with_file_name("empty.json");
        let mut receipts = emit(&path, b"first output");
        receipts.extend(emit(&alias, b"first output"));
        receipts.extend(emit(&empty, b""));
        let mut first = QueryStore::open_analysis_epoch(&manifest).unwrap();
        // No cached stage owns these files: the manifest must protect even
        // non-cacheable emissions using the ordinary query reachability graph.
        first.publish_analysis_outputs(&receipts).unwrap();
        drop(first);
        let reader = PublishedAnalysisOutputs::open(&manifest).unwrap().unwrap();
        let first_epoch = reader.manifest().epoch.clone();
        assert_eq!(reader.manifest().outputs.len(), 3);
        assert_eq!(reader.read(&empty).unwrap(), Some(Vec::new()));
        fs::remove_file(&path).unwrap();
        fs::remove_file(&alias).unwrap();
        fs::remove_file(&empty).unwrap();
        assert_eq!(reader.read(&path).unwrap().unwrap(), b"first output");
        assert_eq!(reader.read(&alias).unwrap().unwrap(), b"first output");
        assert!(
            reader
                .read(&path.with_file_name("absent"))
                .unwrap()
                .is_none()
        );

        let mut next = QueryStore::open_analysis_epoch(&manifest).unwrap();
        let next_receipts = emit(&path, b"second output");
        assert_eq!(
            PublishedAnalysisOutputs::open(&manifest)
                .unwrap()
                .unwrap()
                .manifest()
                .epoch,
            first_epoch
        );
        next.publish_analysis_outputs(&next_receipts).unwrap();
        let fresh = PublishedAnalysisOutputs::open(&manifest).unwrap().unwrap();
        assert_ne!(fresh.manifest().epoch, first_epoch);
        assert_eq!(fresh.manifest().outputs.len(), 1);
        assert_eq!(fresh.read(&path).unwrap().unwrap(), b"second output");
        assert!(fresh.read(&alias).unwrap().is_none());
        fs::remove_file(&path).unwrap();
        #[cfg(target_os = "linux")]
        {
            let old_pack = next.pack_path.clone();
            OpenOptions::new()
                .append(true)
                .open(&old_pack)
                .unwrap()
                .write_all(b"unindexed tail")
                .unwrap();
            next.compact().unwrap();
            assert!(
                !old_pack.exists(),
                "compaction must actually replace the pinned pack"
            );
        }
        assert_eq!(reader.read(&path).unwrap().unwrap(), b"first output");
        assert_eq!(fresh.read(&path).unwrap().unwrap(), b"second output");
        drop(next);
        let reopened = PublishedAnalysisOutputs::open(&manifest).unwrap().unwrap();
        assert_eq!(reopened.read(&path).unwrap().unwrap(), b"second output");
    }

    #[test]
    fn failed_output_manifest_publication_keeps_previous_epoch_and_content() {
        let manifest = super::super::tests::manifest("output-manifest-failed");
        let path = manifest.parent().unwrap().join("generated/output");
        let receipts = emit(&path, b"previous");
        let mut first = QueryStore::open_analysis_epoch(&manifest).unwrap();
        first.publish_analysis_outputs(&receipts).unwrap();
        drop(first);
        let previous = PublishedAnalysisOutputs::open(&manifest).unwrap().unwrap();
        let receipts = emit(&path, b"next");
        let mut failed = QueryStore::open_analysis_epoch(&manifest).unwrap();
        let failed_key = manifest_key(failed.publishing_epoch.as_deref().unwrap());
        let error = failed
            .publish_analysis_outputs_observed(&receipts, || {
                fs::write(&path, b"replacement").unwrap();
            })
            .unwrap_err();
        assert!(error.to_string().contains("changed after emission"));
        let fresh = PublishedAnalysisOutputs::open(&manifest).unwrap().unwrap();
        assert_eq!(fresh.manifest(), previous.manifest());
        assert_eq!(fresh.read(&path).unwrap().unwrap(), b"previous");
        assert!(
            QueryReader::open(&manifest)
                .unwrap()
                .unwrap()
                .get(&failed_key)
                .unwrap()
                .is_none()
        );
        drop(failed);
        let mut next = QueryStore::open_analysis_epoch(&manifest).unwrap();
        next.publish_analysis_outputs(&emit(&path, b"successful"))
            .unwrap();
        assert_eq!(previous.read(&path).unwrap().unwrap(), b"previous");
        assert_eq!(
            PublishedAnalysisOutputs::open(&manifest)
                .unwrap()
                .unwrap()
                .read(&path)
                .unwrap()
                .unwrap(),
            b"successful"
        );
    }

    #[test]
    fn published_streams_have_independent_bounded_cursors_and_outlive_the_manifest() {
        let manifest = super::super::tests::manifest("output-stream-reader");
        let path = manifest.parent().unwrap().join("generated/large");
        let bytes = (0..(INLINE_VALUE_LIMIT * 3))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let receipts = emit(&path, &bytes);
        let mut writer = QueryStore::open_analysis_epoch(&manifest).unwrap();
        writer.publish_analysis_outputs(&receipts).unwrap();
        drop(writer);
        let snapshot = PublishedAnalysisOutputs::open(&manifest).unwrap().unwrap();
        let mut first = snapshot.open_output(&path).unwrap().unwrap();
        let mut second = snapshot.open_output(&path).unwrap().unwrap();
        let mut data = [0; 19];
        first.seek(SeekFrom::Start(7)).unwrap();
        second.seek(SeekFrom::End(-19)).unwrap();
        first.read_exact(&mut data).unwrap();
        assert_eq!(&data, &bytes[7..26]);
        second.read_exact(&mut data).unwrap();
        assert_eq!(&data, &bytes[bytes.len() - 19..]);
        assert_eq!(second.read(&mut data).unwrap(), 0);
        assert!(second.seek(SeekFrom::End(1)).is_err());
        drop(snapshot);
        fs::remove_file(&path).unwrap();
        let mut writer = QueryStore::open(&manifest).unwrap();
        #[cfg(target_os = "linux")]
        {
            let old_pack = writer.pack_path.clone();
            OpenOptions::new()
                .append(true)
                .open(&old_pack)
                .unwrap()
                .write_all(b"orphan")
                .unwrap();
            writer.compact().unwrap();
            assert!(!old_pack.exists());
        }
        drop(writer);
        first.read_exact(&mut data).unwrap();
        assert_eq!(&data, &bytes[26..45]);
        second.seek(SeekFrom::Start(0)).unwrap();
        second.read_exact(&mut data).unwrap();
        assert_eq!(&data, &bytes[..19]);
    }

    #[test]
    fn indexed_ir_accepts_published_cas_views_without_generated_paths() {
        let manifest = super::super::tests::manifest("output-stream-ir");
        let bundle = manifest.parent().unwrap().join("generated/fixture.ir");
        crate::artifacts::write_fixture_bundle(
            &bundle,
            &crate::artifacts::render_linked_ir_fixture(Vec::new(), Vec::new()),
        )
        .unwrap();
        let mut receipts = Vec::new();
        for name in crate::artifacts::BUNDLE_FILES {
            let path = bundle.join(name);
            receipts.extend(emit(&path, &fs::read(&path).unwrap()));
        }
        let mut writer = QueryStore::open_analysis_epoch(&manifest).unwrap();
        writer.publish_analysis_outputs(&receipts).unwrap();
        drop(writer);
        let snapshot = PublishedAnalysisOutputs::open(&manifest).unwrap().unwrap();
        let files = crate::artifacts::BUNDLE_FILES
            .into_iter()
            .map(|name| {
                (
                    name,
                    snapshot.output_view(&bundle.join(name)).unwrap().unwrap(),
                )
            })
            .collect();
        drop(snapshot);
        fs::remove_dir_all(&bundle).unwrap();
        let reader = crate::artifacts::LinkedIrReader::from_files(&bundle, files).unwrap();
        assert!(reader.read_registers().unwrap().is_empty());
        assert!(
            reader
                .read_review_projection()
                .unwrap()
                .functions
                .is_empty()
        );
        assert!(reader.get_function_by_identity("absent").unwrap().is_none());
    }

    #[test]
    fn published_epoch_is_owned_by_its_manifest_even_in_a_shared_cache_directory() {
        let manifest = super::super::tests::manifest("output-manifest-owner");
        let other = manifest.with_file_name("another-project.toml");
        let path = manifest.parent().unwrap().join("generated/output");
        let receipts = emit(&path, b"project-specific output");
        QueryStore::open_analysis_epoch(&manifest)
            .unwrap()
            .publish_analysis_outputs(&receipts)
            .unwrap();
        assert!(PublishedAnalysisOutputs::open(&manifest).unwrap().is_some());
        let error = PublishedAnalysisOutputs::open(&other).err().unwrap();
        assert!(
            error
                .to_string()
                .contains("belongs to another project manifest")
        );
    }

    #[test]
    fn manifest_requires_unique_destinations_and_never_falls_back_to_files() {
        let manifest = super::super::tests::manifest("output-manifest-invalid");
        let path = manifest.parent().unwrap().join("generated/output");
        let receipts = emit(&path, b"exists");
        let mut writer = QueryStore::open_analysis_epoch(&manifest).unwrap();
        let mut duplicates = receipts.clone();
        duplicates.extend(receipts);
        assert!(writer.publish_analysis_outputs(&duplicates).is_err());
        assert!(PublishedAnalysisOutputs::open(&manifest).unwrap().is_none());
        // Low-level epoch fixtures can omit manifests; the public output API
        // must reject that state, despite the generated file being present.
        writer.complete_analysis_epoch().unwrap();
        let error = PublishedAnalysisOutputs::open(&manifest).err().unwrap();
        assert!(error.to_string().contains("no output manifest"));
    }
}
