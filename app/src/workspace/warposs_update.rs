//! Aviso de version nueva de WarpOss.
//!
//! La app no se actualiza sola: consulta el manifiesto publicado junto a los instaladores y, si
//! anuncia una version mayor que la propia, muestra un chip en la barra superior que lleva a la
//! pagina de descarga. Aqui solo vive la logica (descarga, interpretacion y comparacion).

use std::time::Duration;

use serde::Deserialize;
use version_compare::Version;

/// Manifiesto publicado por el equipo junto a los instaladores.
pub const MANIFEST_URL: &str = "https://isoft.css.gob.pa/warposs/version.json";
/// Unico host al que el chip puede llevar: un manifiesto alterado no debe abrir otros sitios.
const TRUSTED_HOST: &str = "isoft.css.gob.pa";
/// Pagina de descarga usada cuando el manifiesto no trae una valida.
const FALLBACK_PAGE: &str = "https://isoft.css.gob.pa/warposs/";

/// Intervalo entre consultas del manifiesto mientras la app esta abierta.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3_600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Un manifiesto real pesa menos de 1 KB; mas que esto no es el manifiesto.
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_NOTES_CHARS: usize = 240;
const MAX_VERSION_CHARS: usize = 32;

/// Version de esta compilacion, la misma que se publica en el manifiesto al liberarla.
pub fn current_version() -> &'static str {
    include_str!("../../channels/oss/VERSION").trim()
}

/// Una version publicada mas nueva que la instalada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: String,
    /// Pagina de descarga, siempre https en el host de confianza.
    pub page_url: String,
    pub notes: Option<String>,
}

impl AvailableUpdate {
    /// Texto del chip: `Actualización 1.2.0`.
    pub fn label(&self) -> String {
        format!("Actualización {}", self.version)
    }

    /// Lineas del tooltip del chip.
    pub fn tooltip_lines(&self, current: &str) -> Vec<String> {
        let mut lines = vec![format!(
            "Hay una versión nueva de WarpOss: {} (tienes la {current}).",
            self.version
        )];
        if let Some(notes) = &self.notes {
            lines.push(notes.clone());
        }
        lines.push("Clic para abrir la página de descarga.".to_owned());
        lines
    }
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    pagina: Option<String>,
    #[serde(default)]
    notas: Option<String>,
}

/// Interpreta el manifiesto y devuelve la actualizacion solo si su version es valida y mayor que
/// `current`. Cualquier otra cosa (JSON invalido, version igual, menor o ilegible) es `None`.
pub fn parse_update(manifest_json: &str, current: &str) -> Option<AvailableUpdate> {
    let manifest: Manifest = serde_json::from_str(manifest_json).ok()?;
    let version = manifest.version.trim();
    if version.is_empty()
        || version.chars().count() > MAX_VERSION_CHARS
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
    {
        return None;
    }
    if Version::from(version)? <= Version::from(current)? {
        return None;
    }
    Some(AvailableUpdate {
        version: version.to_owned(),
        page_url: manifest
            .pagina
            .filter(|page| is_trusted_url(page))
            .unwrap_or_else(|| FALLBACK_PAGE.to_owned()),
        notes: manifest
            .notas
            .map(|notes| clean_text(&notes))
            .filter(|notes| !notes.is_empty()),
    })
}

fn is_trusted_url(candidate: &str) -> bool {
    url::Url::parse(candidate)
        .is_ok_and(|parsed| parsed.scheme() == "https" && parsed.host_str() == Some(TRUSTED_HOST))
}

fn clean_text(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NOTES_CHARS)
        .collect()
}

/// Descarga el manifiesto y lo compara con la version de esta compilacion. Sin red, fuera de la
/// red interna o ante cualquier error devuelve `None`: no saber no es un aviso.
pub async fn check_for_update() -> Option<AvailableUpdate> {
    let current = current_version();
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(format!("WarpOss/{current}"))
        .build()
        .ok()?;
    let response = client.get(MANIFEST_URL).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.bytes().await.ok()?;
    if body.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    parse_update(std::str::from_utf8(&body).ok()?, current)
}

#[cfg(test)]
#[path = "warposs_update_tests.rs"]
mod tests;
