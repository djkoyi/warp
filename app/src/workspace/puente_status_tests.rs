use super::*;

#[test]
fn puente_parse_counts_equipos_y_en_uso() {
    let json = br#"[
        {"Code":"A1","Host":"srv-a","OSName":"Ubuntu","Network":"tailnet","InUse":true,"Caps":{}},
        {"Code":"B2","Host":"srv-b","OSName":"Windows","Network":"tailnet","InUse":false,"Caps":{}},
        {"Code":"C3","Host":"srv-c","OSName":"macOS","Network":"tailnet","InUse":false,"Caps":{}}
    ]"#;
    let status = parse_puente_output(json);

    assert_eq!(status.equipos_count(), 3);
    assert_eq!(status.in_use_count(), 1);
    assert!(!status.is_dimmed());
    assert_eq!(status.label(), "puente: 3 equipos · 1 en uso");
}

#[test]
fn puente_label_singular_sin_uso() {
    let json = br#"[{"Code":"A1","Host":"srv-a","OSName":"Ubuntu","InUse":false}]"#;
    let status = parse_puente_output(json);

    assert_eq!(status.equipos_count(), 1);
    assert_eq!(status.in_use_count(), 0);
    assert!(!status.is_dimmed());
    assert_eq!(status.label(), "puente: 1 equipo");
}

#[test]
fn puente_array_vacio_es_sin_equipos_atenuado() {
    let status = parse_puente_output(b"[]");

    assert!(matches!(status, PuenteStatus::Ok(ref equipos) if equipos.is_empty()));
    assert_eq!(status.equipos_count(), 0);
    assert!(status.is_dimmed());
    assert_eq!(status.label(), "puente: sin equipos");
    assert_eq!(status.tooltip_lines(), vec!["Sin equipos conectados".to_string()]);
}

#[test]
fn puente_json_invalido_es_sin_conexion() {
    let status = parse_puente_output(b"no soy json");

    assert_eq!(status, PuenteStatus::Unavailable);
    assert!(status.is_dimmed());
    assert_eq!(status.label(), "puente: sin conexión");
    assert_eq!(
        status.tooltip_lines(),
        vec!["No se pudo consultar el broker puente".to_string()]
    );
}

#[test]
fn puente_salida_vacia_es_sin_conexion() {
    assert_eq!(parse_puente_output(b""), PuenteStatus::Unavailable);
    assert_eq!(parse_puente_output(b"   \n"), PuenteStatus::Unavailable);
}

#[test]
fn puente_tooltip_lista_equipos_y_resume_el_resto() {
    let mut equipos = Vec::new();
    for i in 0..8 {
        equipos.push(PuenteEquipo {
            code: format!("C{i}"),
            host: format!("host-{i}"),
            os_name: "Linux".to_string(),
            in_use: i == 0,
        });
    }
    let status = PuenteStatus::Ok(equipos);
    let lines = status.tooltip_lines();

    assert_eq!(lines.len(), MAX_TOOLTIP_EQUIPOS + 1);
    assert_eq!(lines[0], "C0 - host-0 (Linux) [en uso]");
    assert_eq!(lines[1], "C1 - host-1 (Linux) [libre]");
    assert_eq!(lines[MAX_TOOLTIP_EQUIPOS], "(+2 más)");
}
