<p align="center"><img src="docs/brand/banner.svg" alt="VIniVIdiVIci — código de consola, ejecución en PC" width="1200" /></p>
<p align="center"><strong>Una capa de compatibilidad experimental para Xbox Series en Windows, centrada en NVIDIA.</strong></p>
<p align="center"><a href="README.md">English</a> · <strong>Español</strong></p>

---

## El proyecto

**VIniVIdiVIci** reúne correcciones de compatibilidad, comprueba su comportamiento en una **RTX 4060 Ti** y documenta su procedencia y sus límites. El nombre le da su propio **VI** a cada parte de *veni, vidi, vici*.

Partimos de [victory-fork](https://github.com/fxceless/victory-fork) y [VIctory](https://github.com/Marvisak/VIctory). El código x86-64 del título se ejecuta de forma nativa en la CPU; los programas de la GPU de consola se traducen para Direct3D 12.

> **Estado: experimental. Llega al mundo del juego a unos 3-5 fotogramas por segundo en nuestro equipo.** La [primera medida dentro del juego](#primera-medida-dentro-del-juego) está más abajo, con sus límites. No es jugable y no atribuimos ninguna mejora de FPS a los cambios de este fork. Los resultados de otros proyectos corresponden a sus versiones y equipos.

El repositorio no incluye archivos del juego, sus recursos ni material del SDK de consola. Debes aportar tus propios archivos y guardarlos fuera del repositorio. Es un proyecto comunitario independiente, sin afiliación con fabricantes o editoras.

## Qué aporta

| Área | Corrección | Procedencia |
| :--- | :--- | :--- |
| **64 lanes → 32 hilos** | Ejecución por parejas de programas compute wave64 en GPU de 32 lanes, junto con los demás cambios de esa PR. | [#854](https://github.com/fxceless/victory-fork/pull/854), PiloteCode |
| **Límites de memoria** | Las lecturas y escrituras anchas que cruzan una página de 64 KiB usan el mapeo de cada página. | [#846](https://github.com/fxceless/victory-fork/pull/846), gomeaw |
| **Píxeles auxiliares** | Corrige `v_readfirstlane` para que las helper lanes completen el bucle probado en la primera pasada. | [0601955](https://github.com/Marvisak/VIctory/commit/0601955361c77fd8e306631784ed084a31e5ad8f), Vax |
| **Actualización de texturas** | Si se desborda la lista de avisos de remapeo, se revisan las texturas colocadas. Conservamos el límite de memoria y sincronizamos la marca de desbordamiento con la lista. | [74315e7](https://github.com/PastiiJay/klbf-lab/commit/74315e765a6a7b24a926e55f9b2a192d6b8ec05a), PastiiJay / KLBF Lab |

Son correcciones de comportamiento con pruebas de regresión. No equivalen a una mejora de FPS demostrada ni a una solución completa del renderizado.

## Compilar y ejecutar

Necesitas Windows 10/11 x64, una CPU con AVX2, una GPU Direct3D 12 con Shader Model 6.6 y mesh shaders para el mundo, [Rust](https://rustup.rs/), las herramientas C++ de MSVC y un Windows SDK. La versión de Rust está fijada en `rust-toolchain.toml`.

```powershell
git clone https://github.com/KomanderDG88/VIniVIdiVIci.git
cd VIniVIdiVIci
Copy-Item victory.example.toml victory.toml
```

Edita `victory.toml` con las rutas de tus archivos, caché y datos guardados, siempre fuera del repositorio. Comprueba las rutas y compila:

```powershell
cargo run -p victory -- config
cargo build --release -p victory
.\target\release\victory.exe run "D:\TuTitulo\titulo.exe" --execute
```

Sustituye la ruta de ejemplo por tu ejecutable. El programa y las variables mantienen sus nombres heredados: **`victory` y `VICTORY_*`**. Deja margen de memoria comprometida disponible en Windows.

Puedes usar un mando XInput. Sin mando, el teclado actúa como uno mientras la ventana tiene el foco: **WASD / IJKL** para los sticks, **Enter** para A, **Backspace** para B y **Esc / Tab** para Menu / View. [Controles completos y guía heredada](README.upstream.md#getting-started).

## Qué hemos comprobado

En Windows, con RTX 4060 Ti y controlador 617.42:

- **Base `1dc8914`:** 247 pruebas de shaders superadas y 2 ignoradas; 170 de GPU superadas y 5 ignoradas.
- **Arreglo de texturas `158676e`:** las 19 pruebas de descriptores superadas. Al desactivar la corrección, la nueva prueba detecta el fallo.
- **Píxeles auxiliares:** 18 de 18 vecinos atendidos tanto en hardware como en WARP.
- **LDS con ejecución por parejas:** dos casos sintéticos repetidos 30 veces, incluido un grupo de 128 hilos.

Algunas rutas de prueba se omiten si falta compilador o dispositivo: el contador de pruebas superadas no garantiza que se hayan ejecutado todas las rutas de hardware. WARP no cubre aquí la ruta por parejas con `WaveSize(32)`.

**Batería de GPU, estabilizada:** los fallos intermitentes de la batería completa de GPU (la liberación de memoria del test de buffers in-place de WARP, una violación de acceso de WARP con tests simultáneos y el orden de grupos del test de presupuesto indirecto) quedan corregidos con fxceless/victory-fork#878 (MamaLeVrai), #875 (eazyves) y un recorte que lanza un grupo por cada 64 registros, a partir del #864 (xiaotu030202). Treinta ejecuciones del binario completo de tests de `victory-gpu` en una RTX 4060 Ti: 30 de 30 limpias (171 superadas, 5 ignoradas), frente a 25 de 30 antes. El cambio del recorte cuesta unos 1,4 µs por cada 64 registros y no es una mejora de fps.

Los comandos de verificación y sus requisitos están en [Validation](README.md#validation). Estas cifras describen pruebas sintéticas, **no rendimiento del juego**.

## Primera medida dentro del juego

Registrada el 25 de noviembre de 2026 en el commit `abf1575`, con **Windows 11, RTX 4060 Ti, controlador 617.42, Core i7-8700K (6 núcleos, 12 hilos) y 32 GB de RAM**, y los archivos del título en un disco duro de 5400 rpm. Compilación de diagnóstico, perfil Series X, `VICTORY_HEAP_SYNC_TABLES=1`, registro de frames y medidor de la tarjeta activados.

| Ejecución | Ventana de 60 s | Frames | Media | Tiempo de frame p5 / p50 / p95 |
| :--- | :--- | ---: | ---: | :--- |
| A | Primera ventana en el mundo | 222 | 3,6 fps | 161 / 262 / 429 ms |
| A | Unos tres minutos después | 188 | 3,0 fps | 115 / 303 / 672 ms |
| B, añadiendo `VICTORY_INDIRECT_WINDOW=1` | Último minuto | 306 | 5,0 fps | 161 / 195 / 242 ms |

- **Llegar al mundo:** el primer frame con 500 draws o más llegó unos 10 minutos después de arrancar. El disco duro limitó la carga: unas 100 lecturas por segundo a 2-4 MB/s en las muestras tomadas, sin descanso.
- **No es una comparación controlada:** la cámara no estaba fija. Las vistas de la ejecución A fueron de unos 530 a 2650 draws por frame y la B se quedó cerca de 800. Con unos 800 draws, un frame habitual tardó 194 ms en A y 201 ms en B: **no atribuimos ninguna mejora a `VICTORY_INDIRECT_WINDOW`**.
- **En qué se va un frame habitual de unos 200 ms:** el hilo de render pasa unos 190 ms dentro de las llamadas de envío, entre 90 y 100 ms preparando y entre 60 y 100 ms esperando a la tarjeta. Copiar de vuelta lo que escriben los shaders lleva de 86 a 141 ms para 15-32 MB, en su propio hilo. La tarjeta está parada de 93 a 134 ms del frame y su trabajo propio es de 73 a 99 ms. En este equipo el frame lo limita el trabajo en serie del procesador, no la tarjeta.
- **Límites:** los contadores de diagnóstico y las capturas ocasionales estaban activos y cuestan tiempo de frame. Se descartaban normalmente entre 80 y 130 draws por frame sin ningún pipeline pendiente, a veces más. Las sombras salían negras en las vistas revisadas. No se probó el control. Las dos ejecuciones las paró la guarda de memoria del propio banco de pruebas (menos de 2 GiB de memoria física libre), no una caída, tras unos 7 y 3 minutos en el mundo.

## Próximos pasos

- Repetir la línea base con los archivos del título en un SSD, para que las lecturas de disco dejen de limitar la carga y el streaming.
- Medir contra esa base los cambios pendientes de sincronización y copia de vuelta, de uno en uno y con un número de draws comparable: fxceless/victory-fork#665, #467, #812, #916, #857 y #853.
- Comparar imagen y estabilidad antes de sacar conclusiones de rendimiento.

## Colaborar y dar crédito

Son bienvenidos los cambios pequeños, con procedencia clara y una prueba reproducible. Incluye en los informes versión, hardware, controlador, ajustes y pasos. Revisa los logs antes de compartirlos y elimina rutas personales y datos privados. Usa casos sintéticos; consulta la [política de archivos](docs/asset-policy.md).

Gracias a **Marvisak y los colaboradores de VIctory**, **fxceless y los colaboradores de victory-fork**, **PiloteCode, gomeaw, Vax y PastiiJay**. Conservamos su historial y atribución. La [documentación heredada](README.upstream.md) mantiene los créditos y notas anteriores; sus cifras históricas no son mediciones de este fork.

La cabecera y el monograma VI son gráficos vectoriales originales creados para VIniVIdiVIci.

**GPL-3.0-or-later** · [Licencia](LICENSE) · [Código](https://github.com/KomanderDG88/VIniVIdiVIci) · [Incidencias](https://github.com/KomanderDG88/VIniVIdiVIci/issues)
