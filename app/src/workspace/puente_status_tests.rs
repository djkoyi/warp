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
    assert_eq!(
        status.tooltip_lines(),
        vec!["Sin equipos conectados".to_string()]
    );
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

#[test]
fn puente_acepta_las_claves_en_minusculas_de_puentectl() {
    let json = br#"[
        {"code":"786123","host":"nb1-denit","os":"windows","os_name":"Microsoft Windows 11",
         "arch":"amd64","user":"CSS\\usuario","caps":["shell"],"network":"CSS","managed":false,
         "since_ms":1795033300521,"in_use":true}
    ]"#;
    let status = parse_puente_output(json);

    assert_eq!(status.label(), "puente: 1 equipo · 1 en uso");
    assert_eq!(
        status.tooltip_lines(),
        vec!["786123 - nb1-denit (Microsoft Windows 11) [en uso]".to_string()]
    );
}

#[test]
fn puente_codigo_valido_solo_admite_caracteres_seguros() {
    assert!(is_valid_code("786123"));
    assert!(is_valid_code("nb1-denit_2"));
    for code in ["", "12 34", "1;rm -rf", "$(id)", "a'b", "../x"] {
        assert!(!is_valid_code(code), "{code}");
    }
    assert!(!is_valid_code(&"9".repeat(65)));
}

#[test]
fn puente_comando_de_conexion_cita_la_ruta_y_rechaza_codigos_raros() {
    let script = Path::new("/Users/yo/Mis Proyectos/puente/bin/puente-ssh.sh");
    assert_eq!(
        connect_command(script, "786123").as_deref(),
        Some("'/Users/yo/Mis Proyectos/puente/bin/puente-ssh.sh' 786123")
    );
    assert_eq!(
        connect_command(Path::new("/a/it's/puente-ssh.sh"), "1").as_deref(),
        Some(r"'/a/it'\''s/puente-ssh.sh' 1")
    );
    assert_eq!(connect_command(script, "1; id"), None);
}

#[test]
fn puente_nombre_de_equipo_para_el_menu() {
    let status = parse_puente_output(br#"[{"code":"786123","host":"nb1-denit"},{"code":"55"}]"#);
    let names: Vec<String> = status
        .equipos()
        .iter()
        .map(PuenteEquipo::display_name)
        .collect();
    assert_eq!(
        names,
        vec!["nb1-denit · 786123".to_string(), "55".to_string()]
    );
    assert!(PuenteStatus::Unavailable.equipos().is_empty());
}
