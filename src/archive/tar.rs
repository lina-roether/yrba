use anyhow::{Context, anyhow};
use flate2::{Compression, write::GzEncoder};
use omnibar::Steps;
use std::path::PathBuf;
use std::{fs, io};
use std::{fs::File, fs::create_dir_all, fs::remove_file, path::Path};
use tar::Builder;

enum ArchiveTarget {
    Link { from: PathBuf, to: PathBuf },
    File { source: PathBuf, dest: PathBuf },
}

impl ArchiveTarget {
    fn append_to<W: io::Write>(&self, builder: &mut tar::Builder<W>) -> anyhow::Result<()> {
        match self {
            Self::Link { from, to } => {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                builder.append_link(&mut header, from, to)?;
            }
            Self::File { source, dest } => {
                builder.append_file(dest, &mut File::open(source)?)?;
            }
        }
        Ok(())
    }
}

fn get_archive_targets(
    base_path: &Path,
    job: &mut omnibar::Job<Steps>,
) -> anyhow::Result<Vec<ArchiveTarget>> {
    let mut targets: Vec<ArchiveTarget> = Vec::new();
    let mut dir_stack: Vec<PathBuf> = vec![base_path.to_path_buf()];
    while let Some(dir) = dir_stack.pop() {
        log::debug!("Enumerating archive targets in {}", dir.display());
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let source = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                dir_stack.push(source);
                continue;
            }

            let dest = source.strip_prefix(base_path)?;

            if file_type.is_symlink() {
                let target = fs::read_link(&source)?;
                targets.push(ArchiveTarget::Link {
                    from: dest.to_path_buf(),
                    to: target.strip_prefix(base_path)?.to_path_buf(),
                });
            } else if file_type.is_file() {
                targets.push(ArchiveTarget::File {
                    source: source.clone(),
                    dest: dest.to_path_buf(),
                });
            } else {
                log::debug!(
                    "Found file of unknown type at {}; skipping...",
                    source.display()
                );
            }
            job.set_num_steps(targets.len());
        }
    }

    Ok(targets)
}

pub(crate) fn create_tarball(
    path_to_backup: &Path,
    temporary_folder_config: Option<String>,
    process: &mut omnibar::MultiProcess,
) -> anyhow::Result<PathBuf> {
    let cache_dir: PathBuf =
        if let Some(temporary_folder_configuration_input) = temporary_folder_config {
            temporary_folder_configuration_input
                .parse()
                .context("Could not parse temporary folder input!")?
        } else {
            get_cache_folder()?
        };
    let mut backup_archive_temp_file_path: PathBuf = cache_dir.join(
        path_to_backup
            .file_name()
            .ok_or_else(|| anyhow!("Could not generate backup file name!"))?,
    );
    backup_archive_temp_file_path.set_extension("tar.gz");
    log::debug!(
        "Creating archive: {}",
        backup_archive_temp_file_path.display()
    );
    create_dir_all(cache_dir).context("Could not create temporary folder for archives!")?;

    let uncompressed_tar: File = File::create(backup_archive_temp_file_path.clone())
        .context("Could not generate filepath for temporary file!")?;

    let enc: GzEncoder<_> = GzEncoder::new(&uncompressed_tar, Compression::default());
    let mut tar: Builder<GzEncoder<_>> = Builder::new(enc);
    tar.follow_symlinks(false);

    let mut final_path_to_backup: &Path = path_to_backup;
    let binding: PathBuf = dirs::home_dir().context("Could not retrieve user home directory!")?;
    let home_dir: &str = binding
        .to_str()
        .context("Could not convert home directory path object to str!")?;
    let replace_dir: &String = &path_to_backup
        .as_os_str()
        .to_str()
        .context("Could not get home directory for input tilde path!")?
        .replace('~', home_dir);
    if path_to_backup.starts_with("~") {
        final_path_to_backup = Path::new(replace_dir);
    }

    let mut archive_job = process.job(
        format!("Archiving {}", final_path_to_backup.display()),
        Steps::unknown(),
    );

    let archive_targets = get_archive_targets(final_path_to_backup, &mut archive_job)
        .context("Failed to enumerate archive targets")?;

    for target in archive_targets {
        if let Err(err) = target.append_to(&mut tar) {
            log::error!(
                "Failed to add file to archive {}\n{err:#}",
                backup_archive_temp_file_path.display()
            );
        } else {
            archive_job.complete_step();
        }
    }

    match tar.finish() {
        Ok(()) => {
            archive_job.complete();
            Ok(backup_archive_temp_file_path)
        }
        Err(err) => {
            archive_job.fail();
            log::error!(
                "Error finalizing tar archive {}\nError: {:?}",
                final_path_to_backup.as_os_str().display(),
                err
            );
            log::info!("Trying to delete faulty temporary archive...");
            // Delete temporary archive
            if remove_file(&backup_archive_temp_file_path).is_err() {
                log::error!(
                    "Could not delete temporary archive! Please manually remove: {}",
                    backup_archive_temp_file_path.display()
                );
            }
            Err(anyhow!(err))
        }
    }
}

fn get_cache_folder() -> anyhow::Result<PathBuf> {
    let cache_dir_parent: PathBuf =
        dirs::cache_dir().context("Could not get temporary directory!")?;
    let cache_dir = cache_dir_parent.join("yrba/");
    Ok(cache_dir)
}
