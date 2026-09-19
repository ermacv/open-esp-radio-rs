use std::path::Path;

use super::{InstallResult, Provider};
use crate::Result;

// Installation uses one persisted journal and immutable, root-owned generations.
// Publishing `current` is the commit point. Stable operational entry points
// trampoline through fixed launchers, which select `current` only after shared
// admission; auxiliary stable paths still resolve through it. Failures before
// the commit restore the previous entries and policy; failures after it either
// verify the new generation, roll back, or retain the journal as an explicit
// recovery-required state. The next apply recovers that journal before starting
// new work, and the previous generation is not removed.
#[cfg(target_os = "linux")]
mod linux {
    use std::{
        fs::{self, File, OpenOptions},
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd as _, OwnedFd},
            unix::{fs::MetadataExt as _, fs::OpenOptionsExt as _},
        },
        path::{Component, Path, PathBuf},
        process::Command,
    };

    use serde::{Deserialize, Serialize};
    use sha2::{Digest as _, Sha256};

    use super::{InstallResult, Provider, Result};
    use crate::fixture_install::{
        Artifact, BUNDLE_SCHEMA, Bundle, InstallState, RECEIPT_SCHEMA,
        model::validate_adapters,
        prepare::{sha256, validate_operator},
    };

    const JOURNAL_SCHEMA: u32 = 1;
    const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

    #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
    enum Phase {
        Imported,
        LinksPrepared,
        PolicyPublished,
        Activated,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Journal {
        schema: u32,
        provider: Provider,
        transaction: String,
        bundle: Bundle,
        previous_generation: Option<String>,
        previous_current: Option<String>,
        phase: Phase,
        stable_backups: Vec<FileBackup>,
        policy_backup: FileBackup,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(rename_all = "kebab-case", tag = "kind", content = "value")]
    enum FileBackup {
        Absent,
        Regular { name: String, mode: u32 },
        Symlink { target: PathBuf },
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum Stage {
        Imported,
        LinksPrepared,
        CandidateValidated,
        PolicyPublished,
        Activated,
        SoftwareVerified,
        Receipt,
        Rollback,
    }

    pub(crate) trait Effects {
        fn checkpoint(&mut self, _stage: Stage) -> Result<()> {
            Ok(())
        }

        fn validate_candidate(&mut self, layout: &Layout, candidate: &Path) -> Result<()>;
        fn validate_effective(&mut self, layout: &Layout) -> Result<()>;
        fn verify_capabilities(&mut self, helper: &Path, expected: &str) -> Result<()>;
        fn require_provider_idle(&mut self, _provider: Provider) -> Result<()> {
            Ok(())
        }
    }

    struct RuntimeEffects;

    impl Effects for RuntimeEffects {
        fn validate_candidate(&mut self, layout: &Layout, candidate: &Path) -> Result<()> {
            let composite = layout.state_root.join("candidate-policy");
            let policy = format!(
                "@include {}\n@include {}\n",
                layout.sudoers_main.display(),
                candidate.display()
            );
            atomic_write(
                &composite,
                policy.as_bytes(),
                0o400,
                layout.expected_uid,
                layout.expected_gid,
            )?;
            let result = run_visudo(layout, &["-c", "-f"], Some(&composite));
            let _ = fs::remove_file(&composite);
            result
        }

        fn validate_effective(&mut self, layout: &Layout) -> Result<()> {
            run_visudo(layout, &["-c"], None)
        }

        fn verify_capabilities(&mut self, helper: &Path, expected: &str) -> Result<()> {
            let output = oer_process::output(
                Command::new(helper).arg("capabilities"),
                Some(std::time::Duration::from_secs(5)),
            )?;
            if !output.status.success() || String::from_utf8(output.stdout)?.trim() != expected {
                return Err(format!(
                    "installed helper failed non-hardware capabilities verification: {}",
                    helper.display()
                )
                .into());
            }
            Ok(())
        }

        fn require_provider_idle(&mut self, provider: Provider) -> Result<()> {
            if provider != Provider::LinuxNet {
                return Ok(());
            }
            let path = Path::new("/run/open-radio-hostapd.pid");
            let mut file = match OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            };
            let metadata = file.metadata()?;
            if !metadata.file_type().is_file()
                || metadata.uid() != 0
                || metadata.gid() != 0
                || metadata.mode() & 0o022 != 0
            {
                return Err("hostapd pid file has unsafe ownership or mode".into());
            }
            let mut pid = String::new();
            file.read_to_string(&mut pid)?;
            let pid: u32 = pid
                .trim()
                .parse()
                .map_err(|_| "hostapd pid file is malformed")?;
            if PathBuf::from("/proc").join(pid.to_string()).exists() {
                return Err("linux-net provider still owns a running hostapd process; finish the active session before installing".into());
            }
            Ok(())
        }
    }

    fn run_visudo(layout: &Layout, args: &[&str], file: Option<&Path>) -> Result<()> {
        let mut command = Command::new(&layout.visudo);
        command.args(args);
        if let Some(file) = file {
            command.arg(file);
        }
        let output = oer_process::output(&mut command, Some(std::time::Duration::from_secs(5)))?;
        if !output.status.success() {
            return Err(format!(
                "visudo rejected fixture policy: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        Ok(())
    }

    #[derive(Clone, Debug)]
    pub(crate) struct Layout {
        root: PathBuf,
        state_root: PathBuf,
        install_lock: PathBuf,
        legacy_session_root: PathBuf,
        sudoers_main: PathBuf,
        visudo: PathBuf,
        expected_uid: u32,
        expected_gid: u32,
    }

    impl Layout {
        fn system(provider: Provider) -> Result<Self> {
            let visudo = ["/usr/sbin/visudo", "/usr/bin/visudo"]
                .into_iter()
                .map(PathBuf::from)
                .find(|path| path.is_file())
                .ok_or("supported visudo was not found at /usr/sbin/visudo or /usr/bin/visudo")?;
            let visudo = fs::canonicalize(visudo)?;
            let metadata = fs::metadata(&visudo)?;
            if !metadata.file_type().is_file()
                || metadata.uid() != 0
                || metadata.gid() != 0
                || metadata.mode() & 0o022 != 0
            {
                return Err("visudo has unsafe ownership or mode".into());
            }
            Ok(Self {
                root: PathBuf::from("/"),
                state_root: PathBuf::from("/var/lib/open-radio/fixture").join(provider.as_str()),
                install_lock: PathBuf::from("/var/lib/open-radio/fixture/install.lock"),
                legacy_session_root: PathBuf::from("/run/open-radio-fixture"),
                sudoers_main: PathBuf::from("/etc/sudoers"),
                visudo,
                expected_uid: 0,
                expected_gid: 0,
            })
        }

        #[cfg(test)]
        pub(crate) fn test(root: &Path, provider: Provider) -> Result<Self> {
            let uid = unsafe { libc::geteuid() };
            let gid = unsafe { libc::getegid() };
            for directory in [
                root.join("var/lib/open-radio/fixture")
                    .join(provider.as_str()),
                root.join("etc/sudoers.d"),
                root.join("usr/local/libexec"),
                root.join("usr/local/sbin"),
            ] {
                fs::create_dir_all(&directory)?;
                fs::set_permissions(&directory, permissions(0o755))?;
            }
            let sudoers_main = root.join("etc/sudoers");
            if !sudoers_main.exists() {
                fs::write(&sudoers_main, b"Defaults env_reset\n")?;
                fs::set_permissions(&sudoers_main, permissions(0o440))?;
            }
            Ok(Self {
                root: root.to_owned(),
                state_root: root
                    .join("var/lib/open-radio/fixture")
                    .join(provider.as_str()),
                install_lock: root.join("var/lib/open-radio/fixture/install.lock"),
                legacy_session_root: root.join("run/open-radio-fixture"),
                sudoers_main,
                visudo: PathBuf::from("/usr/bin/false"),
                expected_uid: uid,
                expected_gid: gid,
            })
        }

        fn generations(&self) -> PathBuf {
            self.state_root.join("generations")
        }

        fn transactions(&self) -> PathBuf {
            self.state_root.join("transactions")
        }

        fn receipts(&self) -> PathBuf {
            self.state_root.join("receipts")
        }

        fn current(&self) -> PathBuf {
            self.state_root.join("current")
        }

        fn journal(&self) -> PathBuf {
            self.state_root.join("transaction.json")
        }

        fn session_lock(&self) -> PathBuf {
            self.state_root.join("session.lock")
        }

        fn legacy_session_lock(&self, provider: Provider) -> PathBuf {
            self.legacy_session_root
                .join(format!("{}.session.lock", provider.as_str()))
        }

        fn policy(&self, provider: Provider) -> PathBuf {
            self.map_absolute(Path::new(provider.policy_path()))
        }

        fn stable_target(&self, artifact: &Artifact) -> PathBuf {
            self.map_absolute(&artifact.target)
        }

        fn map_absolute(&self, path: &Path) -> PathBuf {
            if self.root == Path::new("/") {
                path.to_owned()
            } else {
                self.root
                    .join(path.strip_prefix("/").expect("finite absolute path"))
            }
        }
    }

    pub(crate) fn apply(
        layout: &Layout,
        provider: Provider,
        bundle_path: &Path,
        operator: &str,
        operator_uid: u32,
        effects: &mut impl Effects,
    ) -> Result<InstallResult> {
        validate_operator(operator)?;
        initialize_layout(layout, provider)?;
        let install_lock = open_lock(
            &layout.install_lock,
            0o600,
            layout.expected_uid,
            layout.expected_gid,
        )?;
        fs2::FileExt::try_lock_exclusive(&install_lock).map_err(|error| {
            format!(
                "another fixture installation owns {}: {error}",
                layout.state_root.display()
            )
        })?;
        let _install_lock = ExclusiveLock(install_lock);
        let session_lock = open_lock(
            &layout.session_lock(),
            0o644,
            layout.expected_uid,
            layout.expected_gid,
        )?;
        fs2::FileExt::try_lock_exclusive(&session_lock).map_err(|error| {
            format!("provider {provider} is in use by an active HIL session: {error}")
        })?;
        let _session_lock = ExclusiveLock(session_lock);
        let _legacy_session_lock = acquire_legacy_session_lock(layout, provider)?;
        effects.require_provider_idle(provider)?;

        recover_if_needed(layout, provider, effects)?;
        let bundle = load_bundle(bundle_path, provider, operator, operator_uid)?;
        let transaction = format!("{}-{:08x}", &bundle.generation[..16], std::process::id());
        let existing = classify_existing(layout, provider)?;
        let previous_current = match &existing {
            ExistingInstallation::Versioned(generation) => Some(generation.clone()),
            ExistingInstallation::Fresh | ExistingInstallation::Legacy => None,
        };
        let mut previous_generation = previous_current.clone();
        let policy = policy_bytes(&bundle)?;
        let policy_sha256 = sha256(&policy);
        let mut base = InstallResult {
            schema: RECEIPT_SCHEMA,
            provider,
            transaction: transaction.clone(),
            generation: bundle.generation.clone(),
            source: bundle.source.clone(),
            artifacts: bundle.artifacts.clone(),
            runtime_contract: bundle.runtime_contract.clone(),
            policy_sha256,
            previous_generation: previous_generation.clone(),
            checks: vec![
                "imported-artifact-identity".to_owned(),
                "candidate-effective-policy".to_owned(),
                "published-owner-mode".to_owned(),
                "effective-policy".to_owned(),
                "finite-helper-capabilities".to_owned(),
            ],
            state: InstallState::Prepared,
            changed: previous_generation.as_deref() != Some(&bundle.generation),
            activation_committed: false,
            software_verified: false,
            hardware_acceptance_performed: false,
            primary_error: None,
            recovery_error: None,
        };

        if previous_current.as_deref() == Some(&bundle.generation) {
            verify_installation(layout, &bundle, &policy, effects)?;
            base.state = InstallState::SoftwareVerified;
            base.changed = false;
            base.activation_committed = true;
            base.software_verified = true;
            if let Err(error) = write_receipt(layout, &base) {
                base.state = InstallState::RecoveryRequired;
                base.primary_error = Some(error.to_string());
            }
            return Ok(base);
        }

        let transaction_directory = layout.transactions().join(&transaction);
        if transaction_directory.exists() {
            return Err(format!(
                "unowned transaction directory requires operator recovery: {}",
                transaction_directory.display()
            )
            .into());
        }
        fs::create_dir(&transaction_directory)?;
        fs::set_permissions(&transaction_directory, permissions(0o700))?;
        require_secure_directory(layout, &transaction_directory, 0o700)?;

        import_generation(
            layout,
            &transaction_directory,
            bundle_path,
            &bundle,
            &policy,
            operator_uid,
        )?;
        let stable_backups = backup_stable_targets(layout, &transaction_directory, &bundle)?;
        let policy_backup = backup_file(
            layout,
            &layout.policy(provider),
            &transaction_directory,
            "policy",
        )?;
        if existing == ExistingInstallation::Legacy {
            previous_generation = Some(import_legacy_generation(
                layout,
                &transaction_directory,
                &bundle,
                &stable_backups,
                &policy_backup,
            )?);
        }
        base.previous_generation = previous_generation.clone();
        let mut journal = Journal {
            schema: JOURNAL_SCHEMA,
            provider,
            transaction: transaction.clone(),
            bundle: bundle.clone(),
            previous_generation,
            previous_current,
            phase: Phase::Imported,
            stable_backups,
            policy_backup,
        };
        write_journal(layout, &journal)?;

        let result = (|| -> Result<()> {
            effects.checkpoint(Stage::Imported)?;
            prepare_stable_entries(layout, &journal)?;
            journal.phase = Phase::LinksPrepared;
            write_journal(layout, &journal)?;
            effects.checkpoint(Stage::LinksPrepared)?;

            let candidate = transaction_directory.join("sudoers.candidate");
            atomic_write(
                &candidate,
                &policy,
                0o440,
                layout.expected_uid,
                layout.expected_gid,
            )?;
            require_secure_file(layout, &candidate, 0o440)?;
            effects.validate_candidate(layout, &candidate)?;
            effects.checkpoint(Stage::CandidateValidated)?;
            publish_policy(layout, &journal, &candidate)?;
            effects.validate_effective(layout)?;
            journal.phase = Phase::PolicyPublished;
            write_journal(layout, &journal)?;
            effects.checkpoint(Stage::PolicyPublished)?;

            switch_current(layout, &bundle.generation)?;
            base.activation_committed = true;
            journal.phase = Phase::Activated;
            write_journal(layout, &journal)?;
            effects.checkpoint(Stage::Activated)?;

            verify_installation(layout, &bundle, &policy, effects)?;
            base.software_verified = true;
            effects.checkpoint(Stage::SoftwareVerified)?;
            base.state = InstallState::SoftwareVerified;
            effects.checkpoint(Stage::Receipt)?;
            write_receipt(layout, &base)?;
            clear_journal(layout, &journal)?;
            Ok(())
        })();

        if let Err(error) = result {
            base.primary_error = Some(error.to_string());
            if journal.phase == Phase::Activated && base.state == InstallState::SoftwareVerified {
                base.state = InstallState::RecoveryRequired;
                return Ok(base);
            }
            match rollback(layout, &journal, effects) {
                Ok(()) => {
                    base.state = InstallState::RolledBack;
                    base.activation_committed = false;
                    base.software_verified = false;
                }
                Err(recovery) => {
                    base.state = InstallState::RecoveryRequired;
                    base.recovery_error = Some(recovery.to_string());
                }
            }
        }
        Ok(base)
    }

    fn initialize_layout(layout: &Layout, provider: Provider) -> Result<()> {
        for (directory, mode) in [
            (&layout.state_root, 0o755),
            (&layout.generations(), 0o755),
            (&layout.transactions(), 0o700),
            (&layout.receipts(), 0o755),
        ] {
            create_secure_directory(layout, directory, mode)?;
        }
        for spec in provider.artifact_specs() {
            ensure_destination_parent(
                layout,
                layout
                    .map_absolute(Path::new(spec.target))
                    .parent()
                    .ok_or("artifact target has no parent")?,
            )?;
        }
        ensure_destination_parent(
            layout,
            layout
                .policy(provider)
                .parent()
                .ok_or("policy target has no parent")?,
        )?;
        Ok(())
    }

    fn acquire_legacy_session_lock(
        layout: &Layout,
        provider: Provider,
    ) -> Result<Option<ExclusiveLock>> {
        let path = layout.legacy_session_lock(provider);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        require_secure_directory(layout, &layout.legacy_session_root, 0o755)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|error| {
                format!(
                    "cannot safely open legacy provider lease {}: {error}",
                    path.display()
                )
            })?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file()
            || metadata.uid() != layout.expected_uid
            || metadata.gid() != layout.expected_gid
            || metadata.mode() & 0o777 != 0o644
        {
            return Err(format!(
                "legacy provider lease has unsafe ownership or mode: {}",
                path.display()
            )
            .into());
        }
        fs2::FileExt::try_lock_exclusive(&file).map_err(|error| {
            format!("provider {provider} is in use by a legacy active HIL session: {error}")
        })?;
        Ok(Some(ExclusiveLock(file)))
    }

    fn ensure_destination_parent(layout: &Layout, directory: &Path) -> Result<()> {
        if !directory.exists() {
            ensure_secure_chain(layout, directory, 0o755)?;
        }
        require_existing_secure_parent(layout, directory)
    }

    fn create_secure_directory(layout: &Layout, directory: &Path, mode: u32) -> Result<()> {
        ensure_secure_chain(layout, directory, mode)?;
        require_secure_directory(layout, directory, mode)
    }

    fn ensure_secure_chain(layout: &Layout, destination: &Path, final_mode: u32) -> Result<()> {
        let relative = destination
            .strip_prefix(&layout.root)
            .map_err(|_| "installation destination escapes its fixed root")?;
        let mut current = layout.root.clone();
        for component in relative.components() {
            let Component::Normal(component) = component else {
                return Err("installation destination contains path traversal".into());
            };
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) => {
                    if !metadata.file_type().is_dir()
                        || metadata.uid() != layout.expected_uid
                        || metadata.gid() != layout.expected_gid
                        || metadata.mode() & 0o022 != 0
                    {
                        return Err(format!(
                            "installation path component is unsafe: {}",
                            current.display()
                        )
                        .into());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&current)?;
                    let mode = if current == destination {
                        final_mode
                    } else {
                        0o755
                    };
                    set_owner_mode(&current, layout.expected_uid, layout.expected_gid, mode)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn require_secure_directory(layout: &Layout, path: &Path, expected_mode: u32) -> Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.file_type().is_dir()
            || metadata.uid() != layout.expected_uid
            || metadata.gid() != layout.expected_gid
            || metadata.mode() & 0o777 != expected_mode
            || metadata.mode() & 0o022 != 0
        {
            return Err(format!(
                "insecure root-owned installation directory: {}",
                path.display()
            )
            .into());
        }
        Ok(())
    }

    fn require_secure_file(layout: &Layout, path: &Path, expected_mode: u32) -> Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.file_type().is_file()
            || metadata.uid() != layout.expected_uid
            || metadata.gid() != layout.expected_gid
            || metadata.mode() & 0o777 != expected_mode
        {
            return Err(format!(
                "installed file has wrong type, owner or mode: {}",
                path.display()
            )
            .into());
        }
        Ok(())
    }

    fn open_lock(path: &Path, mode: u32, uid: u32, gid: u32) -> Result<File> {
        let (file, created) = match OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(mode)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => (file, true),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                    .open(path)?,
                false,
            ),
            Err(error) => return Err(error.into()),
        };
        if created {
            fs::set_permissions(path, permissions(mode))?;
        }
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file()
            || metadata.uid() != uid
            || metadata.gid() != gid
            || metadata.mode() & 0o777 != mode
        {
            return Err(format!(
                "installation lock has unsafe ownership or mode: {}",
                path.display()
            )
            .into());
        }
        Ok(file)
    }

    struct ExclusiveLock(File);

    impl Drop for ExclusiveLock {
        fn drop(&mut self) {
            let _ = fs2::FileExt::unlock(&self.0);
        }
    }

    fn load_bundle(
        path: &Path,
        provider: Provider,
        operator: &str,
        operator_uid: u32,
    ) -> Result<Bundle> {
        if !path.is_absolute() {
            return Err("prepared bundle path must be absolute".into());
        }
        let directory = open_absolute_directory(path)?;
        let metadata = directory.metadata()?;
        if metadata.uid() != operator_uid || metadata.mode() & 0o022 != 0 {
            return Err("prepared bundle directory must be owned by the operator and not group/world writable".into());
        }
        let mut manifest = openat_file(directory.as_raw_fd(), "bundle.json")?;
        let manifest_metadata = manifest.metadata()?;
        if manifest_metadata.len() > MAX_MANIFEST_BYTES {
            return Err("prepared bundle manifest is too large".into());
        }
        if manifest_metadata.uid() != operator_uid || manifest_metadata.mode() & 0o022 != 0 {
            return Err("prepared bundle manifest has unsafe ownership or mode".into());
        }
        let mut bytes = Vec::new();
        manifest.read_to_end(&mut bytes)?;
        let bundle: Bundle = serde_json::from_slice(&bytes)?;
        validate_bundle(&bundle, provider, operator)?;
        Ok(bundle)
    }

    fn validate_bundle(bundle: &Bundle, provider: Provider, operator: &str) -> Result<()> {
        if bundle.schema != BUNDLE_SCHEMA
            || bundle.provider != provider
            || bundle.operator != operator
            || bundle.runtime_contract != provider.runtime_contract()
            || bundle.generation.len() != 64
            || !bundle
                .generation
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || bundle.source.commit.len() != 40
            || !bundle
                .source
                .commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || bundle.source.workspace_state_sha256.len() != 64
            || !bundle
                .source
                .workspace_state_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("prepared bundle identity or provider contract is invalid".into());
        }
        if validate_adapters(provider, &bundle.allowed_bluetooth_adapters)?
            != bundle.allowed_bluetooth_adapters
        {
            return Err("prepared bundle Bluetooth adapter policy is not canonical".into());
        }
        let specs = provider.artifact_specs();
        if bundle.artifacts.len() != specs.len() {
            return Err("prepared bundle artifact set is incomplete".into());
        }
        for (artifact, expected) in bundle.artifacts.iter().zip(specs) {
            if artifact.role != expected.role
                || artifact.file_name != expected.name
                || artifact.target != Path::new(expected.target)
                || artifact.mode != expected.mode
                || artifact.sha256.len() != 64
                || !artifact
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(format!(
                    "unexpected artifact declaration for role {:?}",
                    artifact.role
                )
                .into());
            }
        }
        let mut identity = bundle.clone();
        identity.generation.clear();
        if sha256(&serde_json::to_vec(&identity)?) != bundle.generation {
            return Err("prepared bundle generation does not match its manifest".into());
        }
        Ok(())
    }

    fn import_generation(
        layout: &Layout,
        transaction: &Path,
        source_bundle: &Path,
        bundle: &Bundle,
        policy: &[u8],
        operator_uid: u32,
    ) -> Result<PathBuf> {
        let final_path = layout.generations().join(&bundle.generation);
        if final_path.exists() {
            verify_generation(layout, &final_path, bundle)?;
            require_secure_file(layout, &final_path.join("sudoers.policy"), 0o440)?;
            if fs::read(final_path.join("sudoers.policy"))? != policy {
                return Err("installed generation policy identity mismatch".into());
            }
            return Ok(final_path);
        }
        let staging = transaction.join("generation");
        fs::create_dir(&staging)?;
        fs::set_permissions(&staging, permissions(0o700))?;
        let bundle_directory = open_absolute_directory(source_bundle)?;
        let artifacts_directory = openat_directory(bundle_directory.as_raw_fd(), "artifacts")?;
        let artifacts_metadata = artifacts_directory.metadata()?;
        if artifacts_metadata.uid() != operator_uid || artifacts_metadata.mode() & 0o022 != 0 {
            return Err("prepared artifact directory has unsafe ownership or mode".into());
        }
        for artifact in &bundle.artifacts {
            let mut source = openat_file(artifacts_directory.as_raw_fd(), &artifact.file_name)?;
            let metadata = source.metadata()?;
            if !metadata.file_type().is_file()
                || metadata.uid() != operator_uid
                || metadata.mode() & 0o022 != 0
            {
                return Err(
                    format!("bundle artifact is not regular: {}", artifact.file_name).into(),
                );
            }
            let destination = staging.join(&artifact.file_name);
            let mut target = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(&destination)?;
            let mut digest = Sha256::new();
            let mut size = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let count = source.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                target.write_all(&buffer[..count])?;
                digest.update(&buffer[..count]);
                size += count as u64;
            }
            target.sync_all()?;
            if size != artifact.size_bytes || format!("{:x}", digest.finalize()) != artifact.sha256
            {
                return Err(format!(
                    "bundle artifact changed after preparation: {}",
                    artifact.file_name
                )
                .into());
            }
            set_owner_mode(
                &destination,
                layout.expected_uid,
                layout.expected_gid,
                artifact.mode,
            )?;
        }
        atomic_write(
            &staging.join("sudoers.policy"),
            policy,
            0o440,
            layout.expected_uid,
            layout.expected_gid,
        )?;
        fs::rename(&staging, &final_path)?;
        fs::set_permissions(&final_path, permissions(0o555))?;
        sync_parent(&final_path)?;
        verify_generation(layout, &final_path, bundle)?;
        require_secure_file(layout, &final_path.join("sudoers.policy"), 0o440)?;
        Ok(final_path)
    }

    fn verify_generation(layout: &Layout, generation: &Path, bundle: &Bundle) -> Result<()> {
        require_secure_directory(layout, generation, 0o555)?;
        for artifact in &bundle.artifacts {
            let path = generation.join(&artifact.file_name);
            require_secure_file(layout, &path, artifact.mode)?;
            let bytes = fs::read(&path)?;
            if bytes.len() as u64 != artifact.size_bytes || sha256(&bytes) != artifact.sha256 {
                return Err(
                    format!("installed artifact identity mismatch: {}", path.display()).into(),
                );
            }
        }
        Ok(())
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum ExistingInstallation {
        Fresh,
        Legacy,
        Versioned(String),
    }

    fn classify_existing(layout: &Layout, provider: Provider) -> Result<ExistingInstallation> {
        let current = read_current(layout)?;
        let specs = provider.artifact_specs();
        let mut launcher_absent = 0;
        let mut launcher_regular = 0;
        let mut payload_absent = 0;
        let mut payload_regular = 0;
        let mut payload_links = 0;
        for spec in specs {
            let target = layout.map_absolute(Path::new(spec.target));
            match fs::symlink_metadata(&target) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if spec.role.is_launcher() {
                        launcher_absent += 1;
                    } else {
                        payload_absent += 1;
                    }
                }
                Err(error) => return Err(error.into()),
                Ok(metadata) if metadata.file_type().is_file() => {
                    if spec.role.is_launcher() {
                        launcher_regular += 1;
                    } else {
                        payload_regular += 1;
                    }
                }
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    if !spec.role.is_launcher()
                        && fs::read_link(&target)? == layout.current().join(spec.name)
                    {
                        payload_links += 1;
                    } else {
                        return Err(format!(
                            "fixture stable path has an unexpected symlink target: {}",
                            target.display()
                        )
                        .into());
                    }
                }
                Ok(_) => {
                    return Err(format!(
                        "fixture stable path is not regular or a controlled symlink: {}",
                        target.display()
                    )
                    .into());
                }
            }
        }
        let policy_exists = match fs::symlink_metadata(layout.policy(provider)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
            Ok(metadata) if metadata.file_type().is_file() => true,
            Ok(_) => return Err("fixture sudoers policy is not a regular file".into()),
        };
        let launcher_count = specs.iter().filter(|spec| spec.role.is_launcher()).count();
        let payload_count = specs.len() - launcher_count;
        match current {
            Some(generation)
                if payload_links == payload_count
                    && launcher_regular + launcher_absent == launcher_count
                    && policy_exists =>
            {
                Ok(ExistingInstallation::Versioned(generation))
            }
            None if payload_absent == payload_count
                && launcher_absent == launcher_count
                && !policy_exists =>
            {
                Ok(ExistingInstallation::Fresh)
            }
            None if payload_regular == payload_count
                && launcher_absent == launcher_count
                && policy_exists =>
            {
                Ok(ExistingInstallation::Legacy)
            }
            _ => {
                Err("partial or inconsistent fixture installation requires manual recovery".into())
            }
        }
    }

    fn import_legacy_generation(
        layout: &Layout,
        transaction: &Path,
        bundle: &Bundle,
        stable_backups: &[FileBackup],
        policy_backup: &FileBackup,
    ) -> Result<String> {
        let mut digest = Sha256::new();
        for (artifact, backup) in bundle.artifacts.iter().zip(stable_backups) {
            match backup {
                FileBackup::Regular { name, .. } => {
                    digest.update(fs::read(transaction.join(name))?);
                }
                FileBackup::Absent if artifact.role.is_launcher() => {}
                _ => {
                    return Err("legacy installation contains an unexpected artifact".into());
                }
            }
        }
        let FileBackup::Regular {
            name: policy_name, ..
        } = policy_backup
        else {
            return Err("legacy installation is missing its sudoers policy".into());
        };
        digest.update(fs::read(transaction.join(policy_name))?);
        let generation = format!("legacy-{:x}", digest.finalize());
        let destination = layout.generations().join(&generation);
        if destination.exists() {
            return Ok(generation);
        }
        let staging = transaction.join("legacy-generation");
        fs::create_dir(&staging)?;
        fs::set_permissions(&staging, permissions(0o700))?;
        for (artifact, backup) in bundle.artifacts.iter().zip(stable_backups) {
            let FileBackup::Regular { name, mode } = backup else {
                debug_assert!(artifact.role.is_launcher());
                continue;
            };
            let target = staging.join(&artifact.file_name);
            fs::copy(transaction.join(name), &target)?;
            set_owner_mode(&target, layout.expected_uid, layout.expected_gid, *mode)?;
        }
        let target = staging.join("sudoers.policy");
        fs::copy(transaction.join(policy_name), &target)?;
        set_owner_mode(&target, layout.expected_uid, layout.expected_gid, 0o440)?;
        fs::rename(staging, &destination)?;
        fs::set_permissions(&destination, permissions(0o555))?;
        sync_parent(&destination)?;
        Ok(generation)
    }

    fn backup_stable_targets(
        layout: &Layout,
        transaction: &Path,
        bundle: &Bundle,
    ) -> Result<Vec<FileBackup>> {
        let mut states = Vec::new();
        for (index, artifact) in bundle.artifacts.iter().enumerate() {
            states.push(backup_file(
                layout,
                &layout.stable_target(artifact),
                transaction,
                &format!("stable-{index}"),
            )?);
        }
        Ok(states)
    }

    fn backup_file(
        layout: &Layout,
        source: &Path,
        transaction: &Path,
        name: &str,
    ) -> Result<FileBackup> {
        let metadata = match fs::symlink_metadata(source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(FileBackup::Absent);
            }
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() {
            return Ok(FileBackup::Symlink {
                target: fs::read_link(source)?,
            });
        }
        if !metadata.file_type().is_file()
            || metadata.uid() != layout.expected_uid
            || metadata.gid() != layout.expected_gid
            || metadata.mode() & 0o022 != 0
        {
            return Err(
                format!("existing installation file is unsafe: {}", source.display()).into(),
            );
        }
        let backup = transaction.join(name);
        fs::copy(source, &backup)?;
        let mode = metadata.mode() & 0o777;
        set_owner_mode(&backup, layout.expected_uid, layout.expected_gid, mode)?;
        Ok(FileBackup::Regular {
            name: name.to_owned(),
            mode,
        })
    }

    fn prepare_stable_entries(layout: &Layout, journal: &Journal) -> Result<()> {
        for artifact in &journal.bundle.artifacts {
            let target = layout.stable_target(artifact);
            let parent = target.parent().ok_or("stable target has no parent")?;
            require_existing_secure_parent(layout, parent)?;
            if artifact.role.is_launcher() {
                let bytes = fs::read(
                    layout
                        .generations()
                        .join(&journal.bundle.generation)
                        .join(&artifact.file_name),
                )?;
                let temporary = parent.join(format!(
                    ".open-radio-launcher-{}.{}",
                    journal.provider, journal.transaction
                ));
                atomic_write(
                    &temporary,
                    &bytes,
                    artifact.mode,
                    layout.expected_uid,
                    layout.expected_gid,
                )?;
                fs::rename(&temporary, &target)?;
                sync_parent(&target)?;
            } else {
                let link_target = layout.current().join(&artifact.file_name);
                atomic_symlink(&link_target, &target, &journal.transaction)?;
            }
        }
        Ok(())
    }

    fn publish_policy(layout: &Layout, journal: &Journal, candidate: &Path) -> Result<()> {
        let policy = layout.policy(journal.provider);
        let parent = policy.parent().ok_or("policy path has no parent")?;
        require_existing_secure_parent(layout, parent)?;
        let temporary = parent.join(format!(
            ".open-radio-{}.{}",
            journal.provider, journal.transaction
        ));
        let bytes = fs::read(candidate)?;
        atomic_write(
            &temporary,
            &bytes,
            0o440,
            layout.expected_uid,
            layout.expected_gid,
        )?;
        require_secure_file(layout, &temporary, 0o440)?;
        fs::rename(&temporary, &policy)?;
        sync_parent(&policy)
    }

    fn switch_current(layout: &Layout, generation: &str) -> Result<()> {
        let target = PathBuf::from("generations").join(generation);
        atomic_symlink(&target, &layout.current(), generation)
    }

    fn verify_installation(
        layout: &Layout,
        bundle: &Bundle,
        policy: &[u8],
        effects: &mut impl Effects,
    ) -> Result<()> {
        let current = read_current(layout)?;
        if current.as_deref() != Some(bundle.generation.as_str()) {
            return Err("active generation changed during verification".into());
        }
        let generation = layout.generations().join(&bundle.generation);
        verify_generation(layout, &generation, bundle)?;
        for artifact in &bundle.artifacts {
            let target = layout.stable_target(artifact);
            let metadata = fs::symlink_metadata(&target)?;
            if artifact.role.is_launcher() {
                if !metadata.file_type().is_file()
                    || metadata.uid() != layout.expected_uid
                    || metadata.gid() != layout.expected_gid
                    || metadata.mode() & 0o777 != artifact.mode
                    || sha256(&fs::read(&target)?) != artifact.sha256
                {
                    return Err(format!(
                        "stable fixture launcher differs from the committed artifact: {}",
                        target.display()
                    )
                    .into());
                }
            } else if !metadata.file_type().is_symlink()
                || fs::read_link(&target)? != layout.current().join(&artifact.file_name)
            {
                return Err(format!(
                    "stable fixture path does not select current generation: {}",
                    target.display()
                )
                .into());
            }
        }
        let active_policy = layout.policy(bundle.provider);
        require_secure_file(layout, &active_policy, 0o440)?;
        if fs::read(&active_policy)? != policy {
            return Err("active fixture policy differs from the validated candidate".into());
        }
        effects.validate_effective(layout)?;
        let helper_name = match bundle.provider {
            Provider::LinuxNet => "open-radio-net",
            Provider::LinuxBluetooth => "open-radio-bluetooth",
        };
        effects.verify_capabilities(&generation.join(helper_name), &bundle.runtime_contract)
    }

    fn recover_if_needed(
        layout: &Layout,
        provider: Provider,
        effects: &mut impl Effects,
    ) -> Result<()> {
        let journal_path = layout.journal();
        match fs::symlink_metadata(&journal_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(_) => require_secure_file(layout, &journal_path, 0o600)?,
        }
        let bytes = fs::read(&journal_path)?;
        let journal: Journal = serde_json::from_slice(&bytes)?;
        if journal.schema != JOURNAL_SCHEMA || journal.provider != provider {
            return Err("unsupported persisted fixture transaction journal".into());
        }
        if journal.phase == Phase::Activated {
            let policy = policy_bytes(&journal.bundle)?;
            match verify_installation(layout, &journal.bundle, &policy, effects) {
                Ok(()) => {
                    let result =
                        result_from_journal(&journal, InstallState::SoftwareVerified, None, None);
                    write_receipt(layout, &result)?;
                    clear_journal(layout, &journal)?;
                    return Ok(());
                }
                Err(primary) => {
                    rollback(layout, &journal, effects).map_err(|recovery| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("interrupted activated transaction failed verification ({primary}) and rollback ({recovery})").into()
                    })?;
                    return Ok(());
                }
            }
        }
        rollback(layout, &journal, effects)
    }

    fn rollback(layout: &Layout, journal: &Journal, effects: &mut impl Effects) -> Result<()> {
        effects.checkpoint(Stage::Rollback)?;
        if let Some(previous) = &journal.previous_current {
            switch_current(layout, previous)?;
        } else if fs::symlink_metadata(layout.current()).is_ok() {
            fs::remove_file(layout.current())?;
            sync_parent(&layout.current())?;
        }
        let transaction = layout.transactions().join(&journal.transaction);
        for (index, (artifact, backup)) in journal
            .bundle
            .artifacts
            .iter()
            .zip(&journal.stable_backups)
            .enumerate()
        {
            restore_backup(
                layout,
                &layout.stable_target(artifact),
                &transaction,
                backup,
                &format!("restore-{index}"),
            )?;
        }
        restore_backup(
            layout,
            &layout.policy(journal.provider),
            &transaction,
            &journal.policy_backup,
            "restore-policy",
        )?;
        effects.validate_effective(layout)?;
        clear_journal(layout, journal)
    }

    fn restore_backup(
        layout: &Layout,
        target: &Path,
        transaction: &Path,
        backup: &FileBackup,
        temporary_name: &str,
    ) -> Result<()> {
        let parent = target.parent().ok_or("restore target has no parent")?;
        let temporary = parent.join(format!(".{temporary_name}"));
        let _ = fs::remove_file(&temporary);
        match backup {
            FileBackup::Absent => {
                if fs::symlink_metadata(target).is_ok() {
                    fs::remove_file(target)?;
                }
            }
            FileBackup::Symlink { target: link } => {
                std::os::unix::fs::symlink(link, &temporary)?;
                fs::rename(&temporary, target)?;
            }
            FileBackup::Regular { name, mode } => {
                fs::copy(transaction.join(name), &temporary)?;
                set_owner_mode(&temporary, layout.expected_uid, layout.expected_gid, *mode)?;
                fs::rename(&temporary, target)?;
            }
        }
        sync_parent(target)
    }

    fn read_current(layout: &Layout) -> Result<Option<String>> {
        let metadata = match fs::symlink_metadata(layout.current()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_symlink() {
            return Err("active fixture generation selector is not a symlink".into());
        }
        let target = fs::read_link(layout.current())?;
        let mut components = target.components();
        if components.next() != Some(Component::Normal("generations".as_ref())) {
            return Err("active fixture generation selector has an unexpected target".into());
        }
        let Some(Component::Normal(generation)) = components.next() else {
            return Err("active fixture generation selector has no generation".into());
        };
        if components.next().is_some() {
            return Err("active fixture generation selector traverses outside generations".into());
        }
        let generation = generation
            .to_str()
            .ok_or("active generation is not UTF-8")?
            .to_owned();
        require_secure_directory(layout, &layout.generations().join(&generation), 0o555)?;
        Ok(Some(generation))
    }

    pub(crate) fn policy_bytes(bundle: &Bundle) -> Result<Vec<u8>> {
        let operator = &bundle.operator;
        validate_operator(operator)?;
        let mut lines = Vec::new();
        match bundle.provider {
            Provider::LinuxNet => {
                lines.push(format!(
                    "{operator} ALL=(root) NOPASSWD: /usr/local/libexec/open-radio-probe \"\""
                ));
                for operation in [
                    "ap",
                    "capabilities",
                    "identity",
                    "client",
                    "observer",
                    "monitor",
                    "monitor-1",
                    "monitor-6",
                    "monitor-11",
                    "managed",
                    "stop",
                    "status",
                    "usb-reset",
                    "kernel-log",
                ] {
                    lines.push(format!(
                        "{operator} ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net {operation}"
                    ));
                }
            }
            Provider::LinuxBluetooth => {
                for adapter in &bundle.allowed_bluetooth_adapters {
                    lines.push(format!(
                        "{operator} ALL=(root) NOPASSWD: /usr/local/libexec/open-radio-bluetooth check --adapter {adapter}"
                    ));
                    lines.push(format!(
                        "{operator} ALL=(root) NOPASSWD: /usr/local/libexec/open-radio-bluetooth check --adapter {adapter} --dtm-version v1"
                    ));
                    lines.push(format!(
                        "{operator} ALL=(root) NOPASSWD: /usr/local/libexec/open-radio-bluetooth connect-reset --adapter {adapter} --peer * --hold-ms * --termination *"
                    ));
                    for failure in [
                        "missing-key",
                        "wrong-key",
                        "missing-refresh-key",
                        "active-data-mic",
                    ] {
                        lines.push(format!(
                            "{operator} ALL=(root) NOPASSWD: /usr/local/libexec/open-radio-bluetooth security-failure --adapter {adapter} --peer * --failure {failure}"
                        ));
                    }
                    lines.push(format!(
                        "{operator} ALL=(root) NOPASSWD: /usr/local/libexec/open-radio-bluetooth security-failure --adapter {adapter} --peer * --failure missing-key --read-version-before-disconnect"
                    ));
                }
            }
        }
        Ok(format!("{}\n", lines.join("\n")).into_bytes())
    }

    fn write_journal(layout: &Layout, journal: &Journal) -> Result<()> {
        atomic_json(
            &layout.journal(),
            journal,
            0o600,
            layout.expected_uid,
            layout.expected_gid,
        )
    }

    fn write_receipt(layout: &Layout, result: &InstallResult) -> Result<()> {
        atomic_json(
            &layout
                .receipts()
                .join(format!("{}.json", result.generation)),
            result,
            0o444,
            layout.expected_uid,
            layout.expected_gid,
        )?;
        atomic_json(
            &layout.state_root.join("receipt.json"),
            result,
            0o444,
            layout.expected_uid,
            layout.expected_gid,
        )
    }

    fn clear_journal(layout: &Layout, journal: &Journal) -> Result<()> {
        match fs::symlink_metadata(layout.journal()) {
            Ok(_) => {
                fs::remove_file(layout.journal())?;
                sync_parent(&layout.journal())?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let transaction = layout.transactions().join(&journal.transaction);
        if transaction.exists() {
            fs::remove_dir_all(transaction)?;
        }
        Ok(())
    }

    fn result_from_journal(
        journal: &Journal,
        state: InstallState,
        primary_error: Option<String>,
        recovery_error: Option<String>,
    ) -> InstallResult {
        let policy_sha256 = policy_bytes(&journal.bundle)
            .map(|bytes| sha256(&bytes))
            .unwrap_or_default();
        InstallResult {
            schema: RECEIPT_SCHEMA,
            provider: journal.provider,
            transaction: journal.transaction.clone(),
            generation: journal.bundle.generation.clone(),
            source: journal.bundle.source.clone(),
            artifacts: journal.bundle.artifacts.clone(),
            runtime_contract: journal.bundle.runtime_contract.clone(),
            policy_sha256,
            previous_generation: journal.previous_generation.clone(),
            checks: vec![
                "imported-artifact-identity".to_owned(),
                "candidate-effective-policy".to_owned(),
                "published-owner-mode".to_owned(),
                "effective-policy".to_owned(),
                "finite-helper-capabilities".to_owned(),
            ],
            state,
            changed: true,
            activation_committed: true,
            software_verified: state == InstallState::SoftwareVerified,
            hardware_acceptance_performed: false,
            primary_error,
            recovery_error,
        }
    }

    fn atomic_json(
        path: &Path,
        value: &impl Serialize,
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(value)?;
        bytes.push(b'\n');
        atomic_write(path, &bytes, mode, uid, gid)
    }

    fn atomic_write(path: &Path, bytes: &[u8], mode: u32, uid: u32, gid: u32) -> Result<()> {
        let parent = path.parent().ok_or("atomic output has no parent")?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("output name is not UTF-8")?;
        let temporary = parent.join(format!(".{name}.new"));
        let _ = fs::remove_file(&temporary);
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(mode)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        set_owner_mode(&temporary, uid, gid, mode)?;
        fs::rename(&temporary, path)?;
        sync_parent(path)
    }

    fn atomic_symlink(target: &Path, path: &Path, identity: &str) -> Result<()> {
        let parent = path.parent().ok_or("symlink path has no parent")?;
        let temporary = parent.join(format!(
            ".open-radio-{}",
            &identity[..identity.len().min(24)]
        ));
        let _ = fs::remove_file(&temporary);
        std::os::unix::fs::symlink(target, &temporary)?;
        fs::rename(&temporary, path)?;
        sync_parent(path)
    }

    fn sync_parent(path: &Path) -> Result<()> {
        File::open(path.parent().ok_or("path has no parent")?)?.sync_all()?;
        Ok(())
    }

    fn set_owner_mode(path: &Path, uid: u32, gid: u32, mode: u32) -> Result<()> {
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
        if unsafe { libc::chown(c_path.as_ptr(), uid, gid) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        fs::set_permissions(path, permissions(mode))?;
        Ok(())
    }

    fn permissions(mode: u32) -> fs::Permissions {
        use std::os::unix::fs::PermissionsExt as _;
        fs::Permissions::from_mode(mode)
    }

    fn require_existing_secure_parent(layout: &Layout, path: &Path) -> Result<()> {
        reject_symlink_components(path)?;
        let metadata = fs::metadata(path)?;
        if !metadata.is_dir()
            || metadata.uid() != layout.expected_uid
            || metadata.gid() != layout.expected_gid
            || metadata.mode() & 0o022 != 0
        {
            return Err(format!(
                "destination parent is not securely owned: {}",
                path.display()
            )
            .into());
        }
        Ok(())
    }

    fn reject_symlink_components(path: &Path) -> Result<()> {
        let mut current = PathBuf::new();
        for component in path.components() {
            current.push(component);
            if matches!(component, Component::RootDir) {
                continue;
            }
            if fs::symlink_metadata(&current)?.file_type().is_symlink() {
                return Err(format!(
                    "installation path contains a symlink: {}",
                    current.display()
                )
                .into());
            }
        }
        Ok(())
    }

    fn open_absolute_directory(path: &Path) -> Result<File> {
        if !path.is_absolute() {
            return Err("bundle directory must be absolute".into());
        }
        let mut descriptor = open_root()?;
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    descriptor = openat_directory(
                        descriptor.as_raw_fd(),
                        name.to_str().ok_or("bundle path is not UTF-8")?,
                    )?;
                }
                _ => return Err("bundle path traversal is not allowed".into()),
            }
        }
        Ok(descriptor)
    }

    fn open_root() -> Result<File> {
        let path = std::ffi::CString::new("/")?;
        let descriptor = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        owned_file(descriptor)
    }

    fn openat_directory(parent: i32, name: &str) -> Result<File> {
        openat(
            parent,
            name,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    }

    fn openat_file(parent: i32, name: &str) -> Result<File> {
        if name.contains('/') || matches!(name, "." | "..") {
            return Err("bundle member name is not finite".into());
        }
        openat(
            parent,
            name,
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    }

    fn openat(parent: i32, name: &str, flags: i32) -> Result<File> {
        let name = std::ffi::CString::new(name)?;
        let descriptor = unsafe { libc::openat(parent, name.as_ptr(), flags) };
        owned_file(descriptor)
    }

    fn owned_file(descriptor: i32) -> Result<File> {
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
        Ok(File::from(descriptor))
    }

    fn verify_operator(operator: &str) -> Result<u32> {
        validate_operator(operator)?;
        if unsafe { libc::geteuid() } != 0 {
            return Err("privileged fixture apply must run as root through sudo".into());
        }
        let sudo_uid: u32 = std::env::var("SUDO_UID")
            .map_err(|_| "SUDO_UID is required")?
            .parse()
            .map_err(|_| "SUDO_UID is invalid")?;
        if sudo_uid == 0 {
            return Err("SUDO_UID must identify a non-root operator".into());
        }
        let output = Command::new("/usr/bin/id")
            .args(["-u", operator])
            .output()?;
        if !output.status.success()
            || String::from_utf8(output.stdout)?.trim() != sudo_uid.to_string()
        {
            return Err("SUDO_USER and SUDO_UID do not identify the same operator".into());
        }
        Ok(sudo_uid)
    }

    pub(super) fn system_apply(
        provider: Provider,
        bundle: &Path,
        operator: &str,
    ) -> Result<InstallResult> {
        let operator_uid = verify_operator(operator)?;
        let layout = Layout::system(provider)?;
        apply(
            &layout,
            provider,
            bundle,
            operator,
            operator_uid,
            &mut RuntimeEffects,
        )
    }

    #[cfg(test)]
    pub(crate) mod test_support {
        pub(crate) use super::{Effects, Layout, Stage, apply, policy_bytes};
    }
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) use linux::test_support;

#[cfg(target_os = "linux")]
pub fn apply_system(provider: Provider, bundle: &Path, operator: &str) -> Result<InstallResult> {
    linux::system_apply(provider, bundle, operator)
}

#[cfg(not(target_os = "linux"))]
pub fn apply_system(_provider: Provider, _bundle: &Path, _operator: &str) -> Result<InstallResult> {
    Err("fixture installation requires Linux".into())
}
