//! Estado del broker "puente" para el chip nativo de la barra superior.
//!
//! El script configurado imprime en stdout un arreglo JSON de sesiones conectadas y devuelve `[]`
//! rapido cuando el broker no responde. Aqui solo se ejecuta y se interpreta su salida; el token y
//! el control los resuelve el propio script.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

/// Intervalo entre sondeos del broker mientras el chip esta habilitado.
pub const PUENTE_POLL_INTERVAL: Duration = Duration::from_secs(20);

/// Tiempo maximo defensivo para una ejecucion del script, por encima del timeout interno del
/// propio script (6s). Si se excede, el resultado se trata como "sin conexion".
pub const PUENTE_HARD_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximo de equipos listados en el tooltip antes de resumir el resto con "(+K mas)".
const MAX_TOOLTIP_EQUIPOS: usize = 6;

/// Un equipo conectado al broker, tal como lo reporta el script.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PuenteEquipo {
    // `puentectl list --json` emite las claves en minusculas; versiones anteriores, en PascalCase.
    #[serde(rename = "Code", alias = "code", default)]
    pub code: String,
    #[serde(rename = "Host", alias = "host", default)]
    pub host: String,
    #[serde(rename = "OSName", alias = "os_name", default)]
    pub os_name: String,
    #[serde(rename = "InUse", alias = "in_use", default)]
    pub in_use: bool,
}

/// Instantanea del estado del broker para pintar el chip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PuenteStatus {
    /// El script corrio y devolvio JSON valido (posiblemente vacio).
    Ok(Vec<PuenteEquipo>),
    /// El script no existe, fallo, dio timeout o no devolvio JSON valido.
    Unavailable,
}

impl Default for PuenteStatus {
    fn default() -> Self {
        PuenteStatus::Unavailable
    }
}

impl PuenteStatus {
    /// Cantidad de equipos conectados (0 cuando no hay conexion).
    pub fn equipos_count(&self) -> usize {
        match self {
            PuenteStatus::Ok(equipos) => equipos.len(),
            PuenteStatus::Unavailable => 0,
        }
    }

    /// Cantidad de equipos actualmente en uso.
    pub fn in_use_count(&self) -> usize {
        match self {
            PuenteStatus::Ok(equipos) => equipos.iter().filter(|e| e.in_use).count(),
            PuenteStatus::Unavailable => 0,
        }
    }

    /// `true` salvo cuando hay al menos un equipo conectado, para pintar el chip en tono atenuado.
    pub fn is_dimmed(&self) -> bool {
        self.equipos_count() == 0
    }

    /// Texto del chip: `puente: N equipos [· M en uso]`, `puente: sin equipos` o
    /// `puente: sin conexion`.
    pub fn label(&self) -> String {
        match self {
            PuenteStatus::Unavailable => "puente: sin conexión".to_string(),
            PuenteStatus::Ok(equipos) if equipos.is_empty() => "puente: sin equipos".to_string(),
            PuenteStatus::Ok(equipos) => {
                let n = equipos.len();
                let plural = if n == 1 { "" } else { "s" };
                let mut label = format!("puente: {n} equipo{plural}");
                let en_uso = self.in_use_count();
                if en_uso > 0 {
                    label.push_str(&format!(" · {en_uso} en uso"));
                }
                label
            }
        }
    }

    /// Lineas del tooltip (espanol, multilinea).
    pub fn tooltip_lines(&self) -> Vec<String> {
        match self {
            PuenteStatus::Unavailable => {
                vec!["No se pudo consultar el broker puente".to_string()]
            }
            PuenteStatus::Ok(equipos) if equipos.is_empty() => {
                vec!["Sin equipos conectados".to_string()]
            }
            PuenteStatus::Ok(equipos) => {
                let mut lines: Vec<String> = equipos
                    .iter()
                    .take(MAX_TOOLTIP_EQUIPOS)
                    .map(|e| {
                        let estado = if e.in_use { "en uso" } else { "libre" };
                        format!("{} - {} ({}) [{}]", e.code, e.host, e.os_name, estado)
                    })
                    .collect();
                if equipos.len() > MAX_TOOLTIP_EQUIPOS {
                    let restantes = equipos.len() - MAX_TOOLTIP_EQUIPOS;
                    lines.push(format!("(+{restantes} más)"));
                }
                lines
            }
        }
    }
}

/// Interpreta la salida del script en una instantanea. Cualquier salida vacia, no UTF-8 o que no
/// sea un arreglo JSON valido se trata como "sin conexion".
pub fn parse_puente_output(stdout: &[u8]) -> PuenteStatus {
    let text = match std::str::from_utf8(stdout) {
        Ok(text) => text.trim(),
        Err(_) => return PuenteStatus::Unavailable,
    };
    if text.is_empty() {
        return PuenteStatus::Unavailable;
    }
    match serde_json::from_str::<Vec<PuenteEquipo>>(text) {
        Ok(equipos) => PuenteStatus::Ok(equipos),
        Err(_) => PuenteStatus::Unavailable,
    }
}

/// Ejecuta el script (bloqueante) y devuelve la instantanea. Pensado para correrse fuera del hilo
/// de UI (por ejemplo, dentro de `tokio::task::spawn_blocking`). Nunca entra en panico: cualquier
/// error al lanzar el proceso o un codigo de salida distinto de cero se traduce a "sin conexion".
pub fn run_puente_script(script_path: &Path) -> PuenteStatus {
    let output = Command::new(script_path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => parse_puente_output(&output.stdout),
        _ => PuenteStatus::Unavailable,
    }
}

#[cfg(test)]
#[path = "puente_status_tests.rs"]
mod tests;
