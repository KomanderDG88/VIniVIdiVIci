<p align="center"><img src="docs/brand/banner.svg" alt="VIniVIdiVIci — código de consola, ejecución en PC" width="1200" /></p>
<p align="center"><strong>Una capa de compatibilidad experimental para Xbox Series en Windows, centrada en NVIDIA.</strong></p>
<p align="center"><a href="README.md">English</a> · <strong>Español</strong></p>

---

## El proyecto

**VIniVIdiVIci** reúne correcciones de compatibilidad, comprueba su comportamiento en una **RTX 4060 Ti** y documenta su procedencia y sus límites. El nombre le da su propio **VI** a cada parte de *veni, vidi, vici*.

Partimos de [victory-fork](https://github.com/fxceless/victory-fork) y [VIctory](https://github.com/Marvisak/VIctory). El código x86-64 del título se ejecuta de forma nativa en la CPU; los programas de la GPU de consola se traducen para Direct3D 12.

> **Estado: experimental. Llega al mundo del juego y el jugador controla al personaje, a unos 3-5 fotogramas por segundo en nuestro equipo.** En [Estado](#estado) está lo que hemos medido y lo que solo está observado. No es jugable y no atribuimos ninguna mejora de FPS a los cambios de este fork. Los resultados de otros proyectos corresponden a sus versiones y equipos.

El repositorio no incluye archivos del juego, sus recursos ni material del SDK de consola. Debes aportar tus propios archivos y guardarlos fuera del repositorio. Es un proyecto comunitario independiente, sin afiliación con fabricantes o editoras.

## Qué aporta

El fork sigue a victory-fork y está fusionado hasta su `79cfdd9` (25 de noviembre de 2026). Esa fusión trae, encendidos por defecto, un contador de fps dibujado en la esquina de la imagen (`VICTORY_FPS_COUNTER=0` lo quita; en el perfil Series S queda debajo del plano de la interfaz donde este es opaco) y lecturas de DirectStorage a páginas físicas que esperan al Submit del título; y, apagados por defecto, `VICTORY_INDIRECT_GATHER=1`, `VICTORY_SHOT`, `VICTORY_COPY_CHANGED=1` y `VICTORY_WINDOW_UPLOAD=1`. Nada de esto está medido todavía en este equipo. Lo de abajo es lo que añadimos encima.

| Área | Corrección | Procedencia |
| :--- | :--- | :--- |
| **64 lanes → 32 hilos** | Ejecución por parejas de programas compute wave64 en GPU de 32 lanes, junto con los demás cambios de esa PR. | [#854](https://github.com/fxceless/victory-fork/pull/854), PiloteCode |
| **Límites de memoria** | Las lecturas y escrituras anchas que cruzan una página de 64 KiB usan el mapeo de cada página. | [#846](https://github.com/fxceless/victory-fork/pull/846), gomeaw |
| **Píxeles auxiliares** | Corrige `v_readfirstlane` para que las helper lanes completen el bucle probado en la primera pasada. | [0601955](https://github.com/Marvisak/VIctory/commit/0601955361c77fd8e306631784ed084a31e5ad8f), Vax |
| **Actualización de texturas** | Si se desborda la lista de avisos de remapeo, se revisan las texturas colocadas. Conservamos el límite de memoria y sincronizamos la marca de desbordamiento con la lista. | [74315e7](https://github.com/PastiiJay/klbf-lab/commit/74315e765a6a7b24a926e55f9b2a192d6b8ec05a), PastiiJay / KLBF Lab |
| **Programas del reescalador y más instrucciones** | Las imágenes FSPX del reescalador FidelityFX se leen como programas nativos, y se traducen varias instrucciones y formas de descriptor que antes se rechazaban. Adaptado a la ejecución por parejas, con una prueba nuestra para las lecturas que siguen a una sección de ola completa. De los 29 programas del reescalador del título, 28 se traducen, validan y crean pipeline en la RTX 4060 Ti; antes, ninguno. No hemos comparado la imagen con y sin este cambio. El perfil Series S lo necesita para dibujar. | [#258](https://github.com/fxceless/victory-fork/pull/258), Ortomanic |
| **Series S: el segundo plano** | En el perfil Series S el título presenta dos planos, su imagen y su interfaz, y solo se mostraba el primero: el selector de perfil, las imágenes de carga y el vídeo inicial salían negros. Ahora el segundo plano se dibuja sobre el primero, y el escalado que pide el título pasa a ser un estirado simple. El tamaño de la descripción de un plano no sale de la documentación de Microsoft; varias personas confirman que funciona con el mismo título. | [#188](https://github.com/fxceless/victory-fork/pull/188), sagouindupm y Filip722 |
| **Texturas en una tarjeta de 8 GB** | `VICTORY_TEXTURE_DROP_MIPS=N` deja fuera de la copia en la tarjeta los N niveles más grandes de cada textura; cada nivel que se deja fuera son tres cuartos de sus bytes. Apagado por defecto. Su autor lo midió en una RTX 4060 Ti de 8 GB; nuestra medida está pendiente. | [#610](https://github.com/fxceless/victory-fork/pull/610), ofc0urse y Filip722 |
| **Copias entre la memoria del título y la tarjeta** | `VICTORY_CENSUS=1` registra, por cada copia de memoria del título en la tarjeta, cuánto se sube, se compara y se trae de vuelta. `VICTORY_SKIP_WC_BACK=1` es un experimento: lo que los shaders escriben en memoria que el título reservó como write-combining se queda en la tarjeta y no se trae de vuelta; sus autores avisan de posibles artefactos y marcan `VICTORY_SKIP_WC_COMPARE=1` como inseguro. Todo apagado por defecto; nuestra medida está pendiente. | [#868](https://github.com/fxceless/victory-fork/pull/868), Chipsrain e iGoldenNinja |
| **Una vista por sección** | `VICTORY_XMEM_PERSISTENT_VIEW=1` mantiene una vista de cada sección de páginas físicas para las escrituras de DirectStorage, en vez de mapear, confirmar y desmapear en cada escritura. Apagado por defecto; sin medir aquí todavía. | [#231](https://github.com/fxceless/victory-fork/pull/231), HailLucifer666 |
| **Pruebas sin nadie delante** | En las builds de diagnóstico, una tecla programada con `VICTORY_KEYS` se mantiene pulsada hasta que las lecturas del título la han visto (una vez por defecto, `VICTORY_KEYS_READS`), en vez de 0,2 s fijos, que a entre uno y tres frames por segundo caían entre dos lecturas. Las pulsaciones pueden repetirse (`150:Enter*3/20`) o mantenerse un tiempo fijo para andar (`430:W~5`, cinco segundos, también con un mando real conectado), y cada una deja una línea en el registro que dice si el título la leyó y cuántas veces. | este fork |
| **Sustituto de la luz indirecta** | Sin trazado de rayos la luz indirecta del título queda en casi nada, y lo que no ilumina el sol sale negro. `VICTORY_AMBIENT=<nivel>` cambia lo que escribe el programa que resuelve esa luz por una constante: con color (`VICTORY_AMBIENT_TINT`), sombreada en pliegues y bajo los objetos a partir de la profundidad de la escena (`VICTORY_AMBIENT_SHADE`), con los reflejos limitados a ella (`VICTORY_AMBIENT_REFLECT`) y ajustable con el juego en marcha desde un fichero en las builds de diagnóstico (`VICTORY_AMBIENT_FILE`). Apagado por defecto aquí, y entonces no cuesta nada; su autor lo usa a 0,6. Sin probar todavía en este equipo. | [VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) de Belobug (beloooo); primera versión a través del [#917](https://github.com/fxceless/victory-fork/pull/917), UwUCammie |
| **Memoria del título en su sitio** | `VICTORY_STRETCH=1`: una ventana de memoria del título que es una sola tira de páginas de una sección la usa la tarjeta a través de una vista nuestra de esa sección, sin copia, sin refresco y sin copia de vuelta; lo que no se puede abrir así se copia como antes. Funciona sin `VICTORY_INPLACE`. Apagado por defecto. Su autor no midió una ganancia clara en una NVIDIA de 16 GB; sin medir aquí todavía. | [#1127](https://github.com/fxceless/victory-fork/pull/1127), FrancescoGelosoEterea, a partir del `VICTORY_STRETCH` de Belobug |
| **Licencia** | Quitamos la emulación de las instrucciones de procesador exclusivas de AMD que victory-fork había portado de XCL, que es GPL-2.0-only: este programa es GPL-3.0-or-later y no puede llevarla. Ese trabajo lo hace ahora el manejador propio de VIctory. Comprobado en un procesador Intel, donde el título depende de ello para arrancar. | [#1057](https://github.com/fxceless/victory-fork/pull/1057), nullexxx |
| **Hilos de pipelines** | `VICTORY_PIPELINE_WORKERS=<n>` y `VICTORY_PIPELINE_NORMAL=1` fijan cuántos hilos crean pipelines y con qué prioridad. Apagados por defecto. | [#955](https://github.com/fxceless/victory-fork/pull/955), LukaManstein |
| **Watchdog del título** | `VICTORY_NO_WATCHDOG=1` salta el watchdog de cuelgues del propio título en una versión conocida, comprobando antes la versión y los bytes que sustituye. Apagado por defecto y sin usar en la medida de abajo. | [#959](https://github.com/fxceless/victory-fork/pull/959), ruvenesh |
| **Pruebas estables** | Tres fallos intermitentes de tests eliminados: los de vigilancia de memoria, el de ficheros y el de esperas de frame. | [#963](https://github.com/fxceless/victory-fork/pull/963) y [#952](https://github.com/fxceless/victory-fork/pull/952), MamaLeVrai; [#978](https://github.com/fxceless/victory-fork/pull/978), avt0m4t48 |

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
- **Con el #258 fusionado:** 252 pruebas de shaders superadas y 2 ignoradas; la prueba nueva de lectura de lane tras la sección, 10 de 10 con 24 de 24 envíos por parejas en cada pasada, y tres mutaciones que la hacen fallar.

Algunas rutas de prueba se omiten si falta compilador o dispositivo: el contador de pruebas superadas no garantiza que se hayan ejecutado todas las rutas de hardware. WARP no cubre aquí la ruta por parejas con `WaveSize(32)`.

**Batería de GPU, estabilizada:** los fallos intermitentes de la batería completa de GPU (la liberación de memoria del test de buffers in-place de WARP, una violación de acceso de WARP con tests simultáneos y el orden de grupos del test de presupuesto indirecto) quedan corregidos con fxceless/victory-fork#878 (MamaLeVrai), #875 (eazyves) y un recorte que lanza un grupo por cada 64 registros, a partir del #864 (xiaotu030202). Treinta ejecuciones del binario completo de tests de `victory-gpu` en una RTX 4060 Ti: 30 de 30 limpias (171 superadas, 5 ignoradas), frente a 25 de 30 antes. El cambio del recorte cuesta unos 1,4 µs por cada 64 registros y no es una mejora de fps. La pasada posterior de victory-fork que recorta de una vez los registros dispersos de cada lista (`VICTORY_INDIRECT_GATHER=1`) lanzaba todos sus grupos a la vez y contaba con el índice antiguo del shader; aquí lanza un grupo por cada 64 registros como las demás pasadas, con un test de 130 registros dispersos que falla si se repone el lanzamiento único. Diez ejecuciones del binario con ello: 10 de 10 limpias (196 superadas, 5 ignoradas).

Los comandos de verificación y sus requisitos están en [Validation](README.md#validation). Estas cifras describen pruebas sintéticas, **no rendimiento del juego**.

## Estado

A 25 de noviembre de 2026, con **Windows 11, RTX 4060 Ti, controlador 617.42, Core i7-8700K y 32 GB de RAM**, y los archivos del título en un SSD NVMe.

| | Estado | Notas |
| :--- | :--- | :--- |
| En el juego | El jugador controla al personaje y el menú de pausa se abre | Lo comunica quien lo ejecuta, a las 18:36, en una sesión en Series S con la compilación `6dc5f86` (el código de `main`): el personaje responde a la entrada. Sin instrumentar. Después, en una sesión sin nadie delante con `d816f3f` y un Enter programado, el título pasó solo del aviso de inicio a la carga, a las escenas iniciales y a una escena interior que muestra el aviso del juego para moverse y mover la cámara; la entrada probada allí a mano, con toques cortos, no dejó ver nada que se distinguiera de una animación. **Confirmado esa misma tarde con entrada programada** (`VICTORY_KEYS`, compilación `7a262f3`, el código de `main`), sin nadie al teclado: la palanca derecha mantenida cuatro segundos giró la cámara, la izquierda mantenida cuatro segundos giró y desplazó al personaje (vistas comparadas antes y después) y el botón de menú abrió la pausa; el registro de la capa recoge cada una de esas pulsaciones como leída por el título. Una cuarta, la palanca hacia atrás, no se leyó y no demuestra nada en ningún sentido. |
| FPS en el mundo | De unos 3 a 5 fps según la escena | Series X, patio, unos 800 draws por frame: 5,0 fps medidos, ese mismo día por la mañana y desde el disco duro. Series S, cinemática inicial, unos 3000 draws: de 2,9 a 3,6 fps medidos (frames habituales de 279 a 343 ms). Con control del jugador: 3,2 fps medidos en el minuto siguiente al aviso (frame mediano de 313 ms), con un trabajo ajeno ocupando el procesador en ese momento. En la escena interior con el aviso de movimiento, casi quieta, con `VICTORY_AMBIENT=0.6` y `VICTORY_TEXTURE_DROP_MIPS=1`: 3,4 fps medidos en 15 s (52 frames en tres lotes, medianas por lote de 287 a 297 ms), una sesión. En esa misma sala, unos 4500 draws por frame, con dos interruptores que van apagados por defecto: `VICTORY_COPY_PLAIN=alt`, turnos de 20 s dentro de una sesión, frame mediano de 284 ms encendido frente a 297 ms apagado (3,45 frente a 3,28 fps; más corto encendido en cuatro parejas de cuatro); `VICTORY_SKIP_WC_BACK=1`, tres sesiones en el orden apagado, encendido, apagado, frame mediano de 297, 261 y 296 ms (3,3, 3,7 y 3,4 fps). Lo que `VICTORY_SKIP_WC_BACK` le hace a la imagen aún no se ha comparado. RTX 4060 Ti. |
| Estabilidad | 15 minutos en una sesión sin caídas | Series S, 924 s hasta el límite de la sesión, con `VICTORY_TEXTURE_ROOM_PERCENT=95`. Otra sesión de 30 minutos también llegó a su límite, pero su cierre no quedó registrado limpiamente y no la contamos. Esa misma tarde, siete sesiones de diez minutos sin nadie delante en Series S con `VICTORY_TEXTURE_DROP_MIPS=1` llegaron todas a su límite sin caída. En Series X dura menos en este equipo: se queda sin memoria física. |
| Imagen | Con `VICTORY_AMBIENT=3` (apagado por defecto) una escena interior queda iluminada en luz y en sombra, con defectos visibles; sin él, el mundo 3D sale casi todo oscuro. Las imágenes de carga, los subtítulos y los avisos se ven | Series S: el selector y las imágenes de carga se ven desde el #188 (medido aquí: dos planos en Series S, uno en Series X) y la sesión sigue hasta el juego. La imagen 3D sale casi negra y sin detalle, y las texturas se ven mal; los subtítulos y avisos del plano de interfaz se leen. Con `VICTORY_AMBIENT=0.6`, en una sesión, se vieron las paredes y una cama de una escena interior, mientras que un plano anterior de esa misma sesión seguía muy oscuro. En esa sala, un mismo encuadre con tres niveles puestos con el juego en marcha: a 3 las paredes, la cama, el pelo y el personaje se ven claramente más que a 0,6; a 12 paredes y personaje se queman. Así que la imagen oscura responde a esa luz, y el nivel que le va a una escena no es un número único. Lo que no arregla: artefactos en el pelo y el lado de una cara que siguió negro a 0,6 con las paredes iluminadas. Series X: sombras negras. |

Cómo medimos: los tiempos de frame que la capa anota cada unos cinco segundos, solo en el mundo y por escena. Las cifras salen de compilaciones de diagnóstico, que cuestan tiempo de frame. Un equipo y unas pocas sesiones.

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

### Esa misma tarde, desde un SSD

Copiamos los archivos del título a un SSD NVMe del mismo equipo. Estas ejecuciones usaron una compilación de prueba, `abf1575` más fxceless/victory-fork#258; el #258 se fusionó en `main` después. Compilación de diagnóstico, `VICTORY_HEAP_SYNC_TABLES=1`.

| Perfil | Cómo fue | Memoria |
| :--- | :--- | :--- |
| Series X | Primera escena 3D de la introducción a los 4,5 minutos aproximadamente. La paró a los 295 s la guarda de memoria del banco de pruebas, no una caída. | 20,1 GiB privados en el proceso; 201 MB de memoria física disponible; 43,8 GiB de memoria comprometible aún libres |
| Series S | 800 s sin caídas, cerrada a propósito. Las escenas de la introducción avanzaron. | 16,7 GiB privados como máximo; 516 MB de memoria física disponible en el peor momento |

- **El límite en este equipo de 32 GB es la memoria física**, con otros programas abiertos. El archivo de paginación no ayuda: nunca faltó memoria comprometible.
- **Imagen, Series S con el #258:** oscura, con geometría y caras incompletas y artefactos temporales fuertes. No hay un plano equivalente sin el #258 con el que comparar, así que de aquí no se concluye nada sobre el #258.
- **No damos FPS de estas ejecuciones.** Un trabajo ajeno ocupaba unos diez de los doce hilos del equipo, la caché de pipelines estaba fría (unos 418 ms de un frame de 580 ms esperando pipelines) y los refrescos de texturas esperaban de 89 a 342 ms por frame el candado que toman los cambios de mapeo. Los 1,4 fotogramas por segundo que marca el registro en el último minuto miden eso, no la capa.
- **El control del jugador sigue sin confirmar.**

## Próximos pasos

La imagen 3D en Series S es mala por lo que parecen tres motivos distintos, que hay que separar con un ajuste cada vez sobre la misma compilación:

- **Oscura:** el constructor de estructuras de trazado de rayos va apagado por defecto (`VICTORY_BVH_BUILDER=1` lo enciende) y la luz indirecta del título sale de los rayos: lo que no ilumina el sol queda negro. Encendido aquí, la sesión acabó en una violación de acceso en el código del título a los 230 s, así que sigue apagado. Una constante en lugar de esa luz ya está como interruptor (`VICTORY_AMBIENT`, del fork de Belobug). Medido en una sala a 0,6, 3 y 12: la imagen se aclara con el nivel y a 12 se quema, así que en esa sala la oscuridad responde a esa luz (una sala y un encuadre: no está demostrado para todo plano oscuro); queda abierto un nivel por escena (o uno que siga la exposición del título), y el lado oscuro de una cara y los artefactos del pelo no son cosa de la luz.
- **Texturas:** una tarjeta de 8 GB casi no tiene sitio para las texturas del título con las reservas por defecto. Con una reserva menor para las superficies de dibujo hubo 3,4 GB de texturas en la tarjeta donde había 2,1 GB, en sesiones que por lo demás no son comparables. Dejando fuera el nivel más grande de cada textura (`VICTORY_TEXTURE_DROP_MIPS=1`, #610), que es lo que dicen que les funciona a otros con tarjetas de 8 GB, siete sesiones de diez minutos llegaron a las escenas 3D y acabaron en su límite de tiempo sin caída, y una lectura dio 4,3 de 7,2 GB de la tarjeta en uso; todavía no hay comparación emparejada de la imagen.
- **Bordes y temblor:** el estirado simple no acumula ni afila. fxceless/victory-fork#798 hace que el título use su propio reescalador.

Después, los FPS. La mayor parte de un frame es copiar memoria del título a través del núcleo, de 81 a 118 ms de unos 300: la copia en sitio y los cambios pendientes de sincronización y copia de vuelta (fxceless/victory-fork#665, #467, #812, #916, #857 y #853) se miden de uno en uno y con un número de draws comparable. En la sala de arriba, sin esos interruptores, el hilo que envía espera a la tarjeta unos 80 ms de cada frame de 297 mientras el hilo de al lado trae de vuelta las copias de la tarjeta, y la tarjeta pasa unos 150 ms sin nada nuestro en cola. Tres frames despiezados con `VICTORY_SHOT` dicen lo mismo del lado de la tarjeta: lo que dibuja el título ocupó 34 ms del reloj de la tarjeta en un frame de 1700 órdenes y 93 ms en uno de 8400, y las pasadas propias de la capa que ponen al día sus copias de la memoria del título y las comparan ocuparon varias veces eso (son frames de captura, que pesan más de lo normal). El muro es mantener las copias, no dibujar; lo siguiente a medir son los caminos sin copia que ya están y van apagados (`VICTORY_INPLACE=1`, y `VICTORY_STRETCH=1`, del fxceless/victory-fork#1127). Medido aquí con `VICTORY_CENSUS=1` (#868), en ventanas de dos a seis frames de las escenas 3D: se traen de vuelta de la tarjeta de 28 a 63 MB por frame, y del 93 al 95 % son de memoria que el título reservó como write-combining. Lo siguiente a medir es `VICTORY_SKIP_WC_BACK=1`, que los deja en la tarjeta, vigilando los artefactos. En este equipo Intel las instrucciones exclusivas de la consola cuestan de 8000 a 12 000 excepciones por segundo en la cinemática inicial, y eso no limita el frame.

## Colaborar y dar crédito

Son bienvenidos los cambios pequeños, con procedencia clara y una prueba reproducible. Incluye en los informes versión, hardware, controlador, ajustes y pasos. Revisa los logs antes de compartirlos y elimina rutas personales y datos privados. Usa casos sintéticos; consulta la [política de archivos](docs/asset-policy.md).

Gracias a **Marvisak y los colaboradores de VIctory**, **fxceless y los colaboradores de victory-fork**, **PiloteCode, gomeaw, Vax y PastiiJay**. Conservamos su historial y atribución. La [documentación heredada](README.upstream.md) mantiene los créditos y notas anteriores; sus cifras históricas no son mediciones de este fork.

La cabecera y el monograma VI son gráficos vectoriales originales creados para VIniVIdiVIci.

**GPL-3.0-or-later** · [Licencia](LICENSE) · [Código](https://github.com/KomanderDG88/VIniVIdiVIci) · [Incidencias](https://github.com/KomanderDG88/VIniVIdiVIci/issues)
