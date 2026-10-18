//! TLS as libpq's `sslmode` describes it. tokio-postgres knows `disable`,
//! `prefer` and `require`, which encrypt without checking the certificate;
//! `verify-ca` (the chain) and `verify-full` (the chain and the host name)
//! are taken out of the connection string here and checked by `Checked`.

use std::path::PathBuf;
use std::sync::Arc;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, RootCertStore};
use tokio_postgres_rustls::MakeRustlsConnect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verify {
    /// Encrypt only, like `prefer` and `require`.
    #[default]
    No,
    /// `verify-ca`: the certificate chains to a trusted root.
    Ca,
    /// `verify-full`: that, and it's for the host connected to.
    Full,
}

/// What a connection string asks of TLS beyond what tokio-postgres reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    pub verify: Verify,
    /// `sslrootcert`: a PEM file, or `system`.
    pub root_cert: Option<String>,
}

/// Where trusted roots come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Roots {
    File(PathBuf),
    /// The operating system's store.
    System,
}

impl Options {
    /// `sslrootcert`, else `~/.postgresql/root.crt` as in libpq, else the system store.
    pub fn roots(&self) -> Roots {
        match self.root_cert.as_deref() {
            Some("system") => Roots::System,
            Some(p) => Roots::File(expand_home(p)),
            None => default_root_file().filter(|p| p.is_file()).map_or(Roots::System, Roots::File),
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).filter(|h| !h.is_empty()).map(PathBuf::from)
}

fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), home()) {
        (Some(rest), Some(h)) => h.join(rest),
        _ => PathBuf::from(p),
    }
}

fn default_root_file() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("postgresql").join("root.crt"))
    } else {
        home().map(|h| h.join(".postgresql").join("root.crt"))
    }
}

/// `url` without what tokio-postgres can't parse, and what that asked for:
/// `sslmode=verify-ca|verify-full` becomes `sslmode=require`, `sslrootcert`
/// is dropped. Works on both `postgresql://…?…` and `key=value` strings; a
/// string it can't read is returned as it is, for tokio-postgres to report.
pub fn split(url: &str) -> (String, Options) {
    let mut opts = Options::default();
    let mut take = |key: &str, value: &str| -> Option<Option<String>> {
        match (key, value) {
            ("sslmode", "verify-ca") => opts.verify = Verify::Ca,
            ("sslmode", "verify-full") => opts.verify = Verify::Full,
            ("sslrootcert", v) => {
                opts.root_cert = Some(v.to_string());
                return Some(None);
            }
            _ => return None,
        }
        Some(Some("require".into()))
    };
    let out = if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        split_uri(url, &mut take)
    } else {
        split_pairs(url, &mut take)
    };
    match out {
        Some(s) => (s, opts),
        None => (url.to_string(), Options::default()),
    }
}

/// `take(key, value)`: `None` keeps the parameter, `Some(None)` drops it,
/// `Some(Some(v))` gives it value `v`.
type Take<'a> = dyn FnMut(&str, &str) -> Option<Option<String>> + 'a;

fn split_uri(url: &str, take: &mut Take) -> Option<String> {
    let Some((base, query)) = url.split_once('?') else {
        return Some(url.to_string());
    };
    let params: Vec<String> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            match take(k, &percent_decode(v)) {
                None => Some(p.to_string()),
                Some(None) => None,
                Some(Some(v)) => Some(format!("{k}={v}")),
            }
        })
        .collect();
    Some(if params.is_empty() { base.to_string() } else { format!("{base}?{}", params.join("&")) })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(x) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(x);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `key=value key='quoted \' value'`, rebuilt with every value quoted.
fn split_pairs(s: &str, take: &mut Take) -> Option<String> {
    let mut out = vec![];
    let mut it = s.chars().peekable();
    loop {
        while it.next_if(|c| c.is_whitespace()).is_some() {}
        if it.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(c) = it.next_if(|&c| c != '=' && !c.is_whitespace()) {
            key.push(c);
        }
        while it.next_if(|c| c.is_whitespace()).is_some() {}
        if it.next() != Some('=') {
            return None;
        }
        while it.next_if(|c| c.is_whitespace()).is_some() {}
        let mut value = String::new();
        if it.next_if_eq(&'\'').is_some() {
            loop {
                match it.next()? {
                    '\'' => break,
                    '\\' => value.push(it.next()?),
                    c => value.push(c),
                }
            }
        } else {
            while let Some(c) = it.next_if(|c| !c.is_whitespace()) {
                value.push(if c == '\\' { it.next()? } else { c });
            }
        }
        let value = match take(&key, &value) {
            None => value,
            Some(None) => continue,
            Some(Some(v)) => v,
        };
        out.push(format!("{key}='{}'", value.replace('\\', "\\\\").replace('\'', "\\'")));
    }
    Some(out.join(" "))
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn load_roots(roots: &Roots) -> Result<RootCertStore, String> {
    let mut store = RootCertStore::empty();
    match roots {
        Roots::File(path) => {
            let err = |e: rustls::pki_types::pem::Error| format!("sslrootcert {}: {e}", path.display());
            for cert in CertificateDer::pem_file_iter(path).map_err(err)? {
                store.add(cert.map_err(err)?).map_err(|e| format!("sslrootcert {}: {e}", path.display()))?;
            }
            if store.is_empty() {
                return Err(format!("sslrootcert {}: no certificates in it", path.display()));
            }
        }
        Roots::System => {
            store.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
            if store.is_empty() {
                return Err("no system root certificates found; set sslrootcert to the server's CA".into());
            }
        }
    }
    Ok(store)
}

/// The TLS connector for `opts`. `name` is the host the certificate must be
/// for when it isn't the one connected to: the database host behind an SSH
/// tunnel, whose local end is 127.0.0.1.
pub fn connector(opts: &Options, name: Option<&str>) -> Result<MakeRustlsConnect, String> {
    let provider = provider();
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous();
    let verifier: Arc<dyn ServerCertVerifier> = match opts.verify {
        Verify::No => Arc::new(NoVerify(provider)),
        Verify::Ca | Verify::Full => {
            let roots = Arc::new(load_roots(&opts.roots())?);
            let inner =
                WebPkiServerVerifier::builder_with_provider(roots, provider).build().map_err(|e| e.to_string())?;
            let name =
                name.map(|n| ServerName::try_from(n.to_string()).map_err(|e| format!("host {n}: {e}"))).transpose()?;
            Arc::new(Checked { inner, name, full: opts.verify == Verify::Full })
        }
    };
    let config = builder.with_custom_certificate_verifier(verifier).with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
}

/// A hint for an error connecting with `opts`, if it's about the certificate.
pub fn hint(opts: &Options, message: &str) -> Option<&'static str> {
    if opts.verify == Verify::No || !message.contains("invalid peer certificate") {
        None
    } else if message.contains("not valid for name") {
        Some("connect by a host name the certificate is for, or use sslmode=verify-ca to check only who signed it")
    } else {
        Some(
            "the server's certificate isn't signed by a trusted CA: point sslrootcert at its CA, \
             or use sslmode=require to encrypt without checking",
        )
    }
}

/// `sslmode=prefer/require`: encrypted, the certificate not checked, like libpq.
#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// `verify-ca` / `verify-full` on top of webpki.
#[derive(Debug)]
struct Checked {
    inner: Arc<WebPkiServerVerifier>,
    /// Checked instead of the host connected to.
    name: Option<ServerName<'static>>,
    /// Whether the name must match; `verify-ca` only checks the chain.
    full: bool,
}

impl ServerCertVerifier for Checked {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let name = self.name.as_ref().unwrap_or(server_name);
        match self.inner.verify_server_cert(end_entity, intermediates, name, ocsp, now) {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) if !self.full => Ok(ServerCertVerified::assertion()),
            r => r,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_modes_are_taken_out() {
        let (url, o) = split("postgresql://u@db.example.com/app?sslmode=verify-full&sslrootcert=%7E%2Fca.pem&x=1");
        assert_eq!(url, "postgresql://u@db.example.com/app?sslmode=require&x=1");
        assert_eq!(o, Options { verify: Verify::Full, root_cert: Some("~/ca.pem".into()) });
        let (url, o) = split("postgresql://db/app?sslrootcert=system&sslmode=verify-ca");
        assert_eq!((url.as_str(), o.verify), ("postgresql://db/app?sslmode=require", Verify::Ca));
        assert_eq!(o.roots(), Roots::System);
        let (url, o) = split("postgresql://db/app?sslmode=require");
        assert_eq!((url.as_str(), o), ("postgresql://db/app?sslmode=require", Options::default()));
        assert_eq!(split("postgresql:///app").0, "postgresql:///app");
    }

    #[test]
    fn key_value_strings() {
        let (s, o) = split("host=db user=u sslmode = verify-full sslrootcert='/etc/my ca.pem' password='it\\'s'");
        assert_eq!(s, r"host='db' user='u' sslmode='require' password='it\'s'");
        assert_eq!(o, Options { verify: Verify::Full, root_cert: Some("/etc/my ca.pem".into()) });
        let parsed: postgres::Config = s.parse().unwrap();
        assert_eq!(parsed.get_password(), Some(&b"it's"[..]));
        assert_eq!(split("host=db sslmode=").0, "host='db' sslmode=''");
        assert_eq!(split("host='unterminated").0, "host='unterminated");
    }

    #[test]
    fn root_files() {
        let o = |r: &str| Options { verify: Verify::Full, root_cert: Some(r.into()) };
        assert_eq!(o("/etc/ca.pem").roots(), Roots::File("/etc/ca.pem".into()));
        if let Some(h) = home() {
            assert_eq!(o("~/ca.pem").roots(), Roots::File(h.join("ca.pem")));
        }
        assert!(load_roots(&Roots::File("/nonexistent/ca.pem".into())).unwrap_err().contains("/nonexistent/ca.pem"));
    }

    #[test]
    fn hints() {
        let full = Options { verify: Verify::Full, root_cert: None };
        let name = "invalid peer certificate: certificate not valid for name \"localhost\"";
        assert!(hint(&full, name).unwrap().contains("verify-ca"));
        assert!(hint(&full, "invalid peer certificate: UnknownIssuer").unwrap().contains("sslrootcert"));
        assert_eq!(hint(&Options::default(), name), None);
        assert_eq!(hint(&full, "password authentication failed"), None);
    }
}
