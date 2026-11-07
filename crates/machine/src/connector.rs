//! Native OS-service connector adapter. Credentials are private files, never argv.
use crate::{
    Error,
    remote::{Connector, ConnectorFuture},
};
use orchestrator_protocol::{ids::InstallationId, remote::RelayLease};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Installs a pinned frpc and delegates process ownership to launchd or systemd.
#[derive(Debug)]
pub struct NativeConnector {
    directory: PathBuf,
    script: &'static str,
}
impl NativeConnector {
    /// Select the private installation directory; no files or processes are changed yet.
    #[must_use]
    pub fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
            script: include_str!("../../../install/relay.sh"),
        }
    }
    async fn apply(&self, id: InstallationId, selected: Option<&str>) -> Result<(), Error> {
        let root = self.root()?;
        let mut command = tokio::process::Command::new("sh");
        command
            .args([
                "-s",
                "--",
                if selected.is_some() { "start" } else { "stop" },
            ])
            .arg(root)
            .arg(id.to_string())
            .arg(selected.unwrap_or(""));
        crate::dependencies::run_installer(command, self.script, false, Duration::from_mins(5))
            .await
            .map_err(|_| Error::Dependency("relay_service_setup_failed"))
    }
    fn root(&self) -> Result<PathBuf, Error> {
        let root = self.directory.join("relay");
        std::fs::create_dir_all(&root)?;
        std::fs::canonicalize(root).map_err(Error::from)
    }
    fn plan(
        &self,
        lease: &RelayLease,
        credential: &str,
        local: &str,
        os: &str,
    ) -> Result<String, Error> {
        let url = reqwest::Url::parse(local).map_err(|_| Error::Invalid("invalid_relay_target"))?;
        let host = url
            .host_str()
            .ok_or(Error::Invalid("invalid_relay_target"))?;
        if url.scheme() != "http"
            || !host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        {
            return Err(Error::Invalid("relay_target_must_be_loopback"));
        }
        let port = url.port().unwrap_or(80);
        let public = reqwest::Url::parse(&lease.public_url)
            .map_err(|_| Error::Invalid("invalid_relay_address"))?;
        if public.scheme() != "https" {
            return Err(Error::Invalid("managed_relay_requires_https"));
        }
        let root = self.root()?;
        let label = format!(
            "com.cuttlefish.relay.{}.{}.{port}",
            lease.installation_id,
            lease.epoch.simple()
        );
        let config = root.join(format!("{label}.json"));
        let body = json!({"serverAddr":public.host_str(),"serverPort":public.port_or_known_default(),"user":lease.installation_id,
            "loginFailExit":false,"log":{"level":"error"},"auth":{"method":"oidc","additionalScopes":["HeartBeats","NewWorkConns"],"oidc":{"clientID":lease.installation_id,"clientSecret":credential,"tokenEndpointURL":format!("{}/api/v1/relay/token",lease.issuer)}},
            "transport":{"protocol":"wss","tls":{"useSystemCa":true},"heartbeatInterval":10,"heartbeatTimeout":30,"poolCount":0},
            "proxies":[{"name":format!("{}.dashboard",lease.epoch),"type":"http","localIP":host.trim_matches(['[',']']),"localPort":port,"customDomains":[format!("{}.{}.cuttlefish.invalid",lease.installation_id,lease.epoch)],"locations":["/"]}]});
        write_once(&root, &config, body.to_string().as_bytes())?;
        let (extension, unit) = service_unit(os, &label, &root.join("frp-0.72.0/frpc"), &config)?;
        write_once(
            &root,
            &root.join(format!("{label}.{extension}")),
            unit.as_bytes(),
        )?;
        Ok(label)
    }
}
impl Connector for NativeConnector {
    fn start<'a>(
        &'a self,
        lease: &'a RelayLease,
        credential: &'a str,
        local: &'a str,
    ) -> ConnectorFuture<'a> {
        Box::pin(async move {
            let label = self.plan(lease, credential, local, std::env::consts::OS)?;
            self.apply(lease.installation_id, Some(&label)).await
        })
    }
    fn stop(&self, id: InstallationId) -> ConnectorFuture<'_> {
        Box::pin(self.apply(id, None))
    }
}
fn write_once(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), Error> {
    match std::fs::read(path) {
        Ok(saved) if saved == bytes => Ok(()),
        Ok(_) => Err(Error::Invalid("relay_service_configuration_conflict")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            crate::files::AtomicFiles::default()
                .write_new(root, path, bytes)
                .map_err(Error::from)
        }
        Err(error) => Err(error.into()),
    }
}
fn service_unit(
    os: &str,
    label: &str,
    binary: &Path,
    config: &Path,
) -> Result<(&'static str, String), Error> {
    let binary = binary
        .to_str()
        .ok_or(Error::Invalid("relay_path_must_be_utf8"))?;
    let config = config
        .to_str()
        .ok_or(Error::Invalid("relay_path_must_be_utf8"))?;
    if binary.contains(['\n', '\r', '\0']) || config.contains(['\n', '\r', '\0']) {
        return Err(Error::Invalid("invalid_relay_path"));
    }
    match os {
        "macos" => Ok((
            "plist",
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array><string>{}</string><string>-c</string><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>5</integer></dict></plist>",
                xml(label),
                xml(binary),
                xml(config)
            ),
        )),
        "linux" => Ok((
            "service",
            format!(
                "[Unit]\nDescription=Cuttlefish relay connector\nAfter=network-online.target\n[Service]\nExecStart={} -c {}\nRestart=always\nRestartSec=5\nNoNewPrivileges=true\n[Install]\nWantedBy=default.target\n",
                systemd(binary),
                systemd(config)
            ),
        )),
        _ => Err(Error::Invalid("managed_relay_requires_macos_or_linux")),
    }
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn systemd(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

#[cfg(test)]
#[path = "../tests/internal/connector.rs"]
mod tests;
