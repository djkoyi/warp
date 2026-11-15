# VERSIÓN CAJA DE SEGURO SOCIAL (FORK DE WARP)

Este repositorio es un fork personal del terminal de código abierto Warp
(https://github.com/warpdotdev/warp), mantenido en https://github.com/djkoyi/warp.
Es una **versión modificada de Warp** y no es un producto oficial de Warp ni cuenta
con su respaldo.

VERSIÓN MODIFICADA DE WARP (AGPL-3.0), NOVIEMBRE DE 2026. CÓDIGO FUENTE: GITHUB.COM/DJKOYI/WARP

## Responsable

- Ing. Franklin H. Rodríguez Palma
- Departamento de Ingeniería de Software
- Caja de Seguro Social (Panamá)

## Licencia

- Los crates `warpui` y `warpui_core` se distribuyen bajo licencia MIT
  (ver `LICENSE-MIT`).
- El resto del código se distribuye bajo AGPL-3.0 (ver `LICENSE-AGPL`).
- Se conservan todos los avisos de derechos de autor del proyecto original
  ("Copyright (C) 2020-2026 Denver Technologies, Inc."). Las modificaciones de este fork
  se distribuyen bajo los mismos términos de licencia que el código al que modifican.
- De conformidad con la AGPL-3.0 (sección 5), este fork declara de forma visible que
  el software fue modificado respecto de la versión original, con la fecha de la
  modificación. El aviso aparece en Settings > About.
- Conforme a la AGPL-3.0 (sección 13), quien interactúe con una versión modificada
  a través de una red tiene derecho a obtener su código fuente correspondiente,
  disponible en https://github.com/djkoyi/warp.

## Alcance de los cambios

Cambios de este fork respecto de Warp (todos con fecha 15 de noviembre de 2026):

- Créditos e identificación del fork en Settings > About, este archivo y el README.
- Dictado nativo de macOS (tecla fn/globe) dentro del terminal.
- IA de Warp y telemetría desactivadas por defecto.
- Actualizaciones automáticas desactivadas.
- Restauración de sesiones de Claude Code por panel al reiniciar Warp.
- Medidores de uso de Claude Code y estado por pestaña.
- Pestañas automáticas por proyecto y espacios de trabajo.
- Revisión de cambios de Claude Code, sesiones en worktrees de git y monitor de PR/CI.
- Tareas programadas de Claude Code y vista previa del servidor de desarrollo.

No se modifican identificadores de paquete, nombres de binarios ni rutas de
configuración del proyecto original.
