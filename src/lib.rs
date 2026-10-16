use std::fs;

use zed_extension_api::{self as zed, LanguageServerId, LanguageServerInstallationStatus, Result};

const REPO: &str = "suyaroff/zed-zpg";
const BIN: &str = "zpg";

struct ZpgExtension {
    cached_binary: Option<String>,
}

fn is_file(path: &str) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file())
}

fn target() -> Result<(&'static str, zed::DownloadedFileType, &'static str)> {
    use zed::{Architecture as A, DownloadedFileType as F, Os};
    Ok(match zed::current_platform() {
        (Os::Linux, A::X8664) => ("x86_64-unknown-linux-gnu", F::GzipTar, ""),
        (Os::Linux, A::Aarch64) => ("aarch64-unknown-linux-gnu", F::GzipTar, ""),
        (Os::Mac, A::X8664) => ("x86_64-apple-darwin", F::GzipTar, ""),
        (Os::Mac, A::Aarch64) => ("aarch64-apple-darwin", F::GzipTar, ""),
        (Os::Windows, A::X8664) => ("x86_64-pc-windows-msvc", F::Zip, ".exe"),
        (os, arch) => return Err(format!("zpg has no prebuilt binary for {os:?}/{arch:?}")),
    })
}

/// A previously downloaded binary, used when GitHub can't be reached.
fn any_downloaded(exe: &str) -> Option<String> {
    fs::read_dir(".")
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("zpg-"))
        .map(|dir| format!("{dir}/{BIN}{exe}"))
        .find(|p| is_file(p))
}

impl ZpgExtension {
    fn binary(&mut self, id: &LanguageServerId, worktree: &zed::Worktree) -> Result<String> {
        if let Some(path) = worktree.which(BIN) {
            return Ok(path);
        }
        if let Some(path) = &self.cached_binary
            && is_file(path)
        {
            return Ok(path.clone());
        }

        let (triple, file_type, exe) = target()?;
        zed::set_language_server_installation_status(id, &LanguageServerInstallationStatus::CheckingForUpdate);
        let release = match zed::latest_github_release(
            REPO,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        ) {
            Ok(r) => r,
            Err(e) => return any_downloaded(exe).ok_or(e),
        };

        let ext = if exe.is_empty() { "tar.gz" } else { "zip" };
        let asset_name = format!("{BIN}-{triple}.{ext}");
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == asset_name)
            .ok_or_else(|| format!("release {} has no asset {asset_name}", release.version))?;

        let dir = format!("{BIN}-{}", release.version);
        let path = format!("{dir}/{BIN}{exe}");
        if !is_file(&path) {
            zed::set_language_server_installation_status(id, &LanguageServerInstallationStatus::Downloading);
            zed::download_file(&asset.download_url, &dir, file_type)
                .map_err(|e| format!("failed to download {asset_name}: {e}"))?;
            zed::make_file_executable(&path)?;
            for entry in fs::read_dir(".").into_iter().flatten().flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with("zpg-") && name.to_string_lossy() != dir {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        zed::set_language_server_installation_status(id, &LanguageServerInstallationStatus::None);
        self.cached_binary = Some(path.clone());
        Ok(path)
    }
}

impl zed::Extension for ZpgExtension {
    fn new() -> Self {
        Self { cached_binary: None }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        Ok(zed::Command {
            command: self.binary(language_server_id, worktree)?,
            args: vec!["lsp".into()],
            env: worktree.shell_env(),
        })
    }

}

zed::register_extension!(ZpgExtension);
