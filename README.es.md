<p align="center"><img src="docs/brand/banner.svg" alt="VIniVIdiVIci — código de consola, ejecución en PC" width="1200" /></p>
<p align="center"><strong>Una capa de compatibilidad experimental para Xbox Series en Windows, centrada en NVIDIA.</strong></p>
<p align="center"><a href="README.md">English</a> · <strong>Español</strong></p>

---

## El proyecto

**VIniVIdiVIci** reúne correcciones de compatibilidad, comprueba su comportamiento en una **RTX 4060 Ti** y documenta su procedencia y sus límites. El nombre le da su propio **VI** a cada parte de *veni, vidi, vici*.

Partimos de [victory-fork](https://github.com/fxceless/victory-fork) y [VIctory](https://github.com/Marvisak/VIctory). El código x86-64 del título se ejecuta de forma nativa en la CPU; los programas de la GPU de consola se traducen para Direct3D 12.

> **Estado: experimental. Llega al mundo del juego y el jugador controla al personaje, a entre 6,2 y 6,7 fotogramas por segundo en la primera sala en nuestro equipo (sesiones guionizadas), con bajones mientras carga una zona.** En [Estado](#estado) está lo que hemos medido y lo que solo está observado. No es jugable y no atribuimos ninguna mejora de FPS a los cambios de este fork. Los resultados de otros proyectos corresponden a sus versiones y equipos.

El repositorio no incluye archivos del juego, sus recursos ni material del SDK de consola. Debes aportar tus propios archivos y guardarlos fuera del repositorio. Es un proyecto comunitario independiente, sin afiliación con fabricantes o editoras.

## Qué aporta

El fork sigue a victory-fork y está fusionado hasta su `e4245f3` (26 de noviembre de 2026), con más de sus pull requests abiertos y de otros forks fusionado el 27 de noviembre. Son correcciones de comportamiento con pruebas de regresión. No equivalen a una mejora de FPS demostrada ni a una solución completa del renderizado. Qué hace cada cambio, qué se ha medido de él aquí y qué trajo cada fusión está en [docs/fork-changes.es.md](docs/fork-changes.es.md); la tabla de abajo nombra cada cambio y de quién es el trabajo.

| Área | Fuente |
| :--- | :--- |
| 64 lanes → 32 hilos | [#854](https://github.com/fxceless/victory-fork/pull/854), PiloteCode |
| Límites de memoria | [#846](https://github.com/fxceless/victory-fork/pull/846), gomeaw |
| Píxeles auxiliares | [0601955](https://github.com/Marvisak/VIctory/commit/0601955361c77fd8e306631784ed084a31e5ad8f), Vax |
| Actualización de texturas | [74315e7](https://github.com/PastiiJay/klbf-lab/commit/74315e765a6a7b24a926e55f9b2a192d6b8ec05a), PastiiJay / KLBF Lab |
| Programas del reescalador y más instrucciones | [#258](https://github.com/fxceless/victory-fork/pull/258), Ortomanic |
| Series S: el segundo plano | [#188](https://github.com/fxceless/victory-fork/pull/188), sagouindupm y Filip722 |
| Texturas en una tarjeta de 8 GB | [#610](https://github.com/fxceless/victory-fork/pull/610), ofc0urse y Filip722 |
| Copias entre la memoria del título y la tarjeta | [#868](https://github.com/fxceless/victory-fork/pull/868), Chipsrain e iGoldenNinja |
| Una vista por sección | [#231](https://github.com/fxceless/victory-fork/pull/231), HailLucifer666 |
| Pruebas sin nadie delante | este fork |
| Sustituto de la luz indirecta | [VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) de Belobug (beloooo); primera versión a través del [#917](https://github.com/fxceless/victory-fork/pull/917), UwUCammie |
| Memoria del título en su sitio | [#1127](https://github.com/fxceless/victory-fork/pull/1127), FrancescoGelosoEterea, a partir del `VICTORY_STRETCH` de Belobug |
| Draws perdidos por un pipeline rechazado | [Marvisak/VIctory#1152](https://github.com/Marvisak/VIctory/pull/1152), Pablo González, portado a este árbol |
| Texturas más allá de la memoria propia de la tarjeta | [VI-BR-PORT](https://github.com/edsuuu/VI-BR-PORT) de edsuuu, a partir del [#601](https://github.com/fxceless/victory-fork/pull/601), DiogoPCastelos |
| La pasada por el montón de descriptores | [#1043](https://github.com/fxceless/victory-fork/pull/1043) y [#1190](https://github.com/fxceless/victory-fork/pull/1190), UwUCammie |
| Ventanas en la tarjeta contadas una vez | [#1250](https://github.com/fxceless/victory-fork/pull/1250), UwUCammie |
| Una textura corta en vez de una negra | LoverCatys/ruff-lab `ac473bf2`, a partir de ka1qux |
| Memoria intermedia de texturas que no cambian, liberada | LoverCatys/ruff-lab `af3322dd` |
| Hilos de consola en grupos de cálculo de varias waves del host | [#1261](https://github.com/fxceless/victory-fork/pull/1261), Garu667 |
| Una vista raw no se sale de su búfer | [#1266](https://github.com/fxceless/victory-fork/pull/1266), lichenlang89 |
| Una vista de búfer constante de cero bytes sigue siéndolo | [#1260](https://github.com/fxceless/victory-fork/pull/1260), fusionado en el original |
| Más huecos dinámicos de descriptores si se piden | trustno1-coder/VIctory-NVIDIA `a9632bc` |
| Copias y rellenos hechos en la tarjeta en vez de esperados | trustno1-coder/VIctory-NVIDIA `cc03ecb`, `007fe55` |
| Las texturas del horneado de la piel, subidas antes que él | [#1264](https://github.com/fxceless/victory-fork/pull/1264), UwUCammie |
| Páginas físicas respaldadas por secciones de 2 MiB | [#1274](https://github.com/fxceless/victory-fork/pull/1274), CuteBooty |
| Las copias de búfer se sueltan con su montón de vistas | [#1276](https://github.com/fxceless/victory-fork/pull/1276), UwUCammie |
| Un orden justo para las texturas que esperan a subirse | ZigZagBoi1/ScarlettPort `8d32f87` (runtime/victory), Ziggy |
| Un tope al sitio guardado para copias en la tarjeta que aún no existen | a partir de ZigZagBoi1/ScarlettPort |
| Hilos propios para los pipelines de las pasadas a pantalla completa | Belobug/VIctory-Belobug-AMD `18e57db`, beloooo |
| DispatchX por número de hilos | [#1292](https://github.com/fxceless/victory-fork/pull/1292), KyleS98 |
| Los pipelines del constructor de estructuras, fabricados por delante | este fork |
| Memoria del título por dirección: escrita solo por un shader que puede escribir por dirección | [#916](https://github.com/fxceless/victory-fork/pull/916), SpaghettiMaster545 |
| Escala de render y reescalador propio del título | trustno1-coder/VIctory-NVIDIA `afd84de` (de ZigZagBoi1/ScarlettPort `runtime/victory`, a través de vitutu1/victory-perf) |
| Instrucciones solo de AMD, contadas | este fork |
| Arreglada una fuga de formas de vistas colocadas | TaynanGT/Vice-Tay PR #27 (`f07e54a`, `ce91490`, `d3626d2`), de JMAN730/project-flamingo `b1e0cd1` |
| Los huecos dinámicos se devuelven solo al montón que los tomó | [#1324](https://github.com/fxceless/victory-fork/pull/1324), [#1333](https://github.com/fxceless/victory-fork/pull/1333) (de Kamiloj9/victory-dlss `2023d5c`, PastiiJay/klbf-lab `74315e7`), [#1336](https://github.com/fxceless/victory-fork/pull/1336), cwWwWwW |
| Los draws de malla los lanza la tarjeta | stellarr-studio/kelvaroq `32c4ab4` (parche 0041) |
| Las pasadas de skinning para el trazado de rayos, fuera | edsuuu/VI-BR-PORT `ab4571d` |
| El coste del constructor de estructuras | este fork; avatarmanz/VIctory-7900XTX `adf5706`, `30f46c9` (de beloooo) |
| Tres interruptores de imagen | miaohaixiang1-netizen/cool-fox-12gb `db556df` (de trustno1-coder/VIctory-NVIDIA); [#683](https://github.com/fxceless/victory-fork/pull/683), Danigamer05; [#907](https://github.com/fxceless/victory-fork/pull/907), SpaghettiMaster545 |
| Sitios de instrucciones solo de AMD, reescritos | este fork |
| Guardas, refrescos y la pasada del montón | [#1315](https://github.com/fxceless/victory-fork/pull/1315), KainanP; este fork; [#1138](https://github.com/fxceless/victory-fork/pull/1138), exotiquee; [#1146](https://github.com/fxceless/victory-fork/pull/1146), beamdotdev |
| Copias que no se repiten y envíos unidos | LooperSalty/AnyXboxX PR #358 (#340, #337), Skyfrye |
| Tres interruptores más, apagados por defecto | Itsbatt/victory-unified `768735a`; [#1312](https://github.com/fxceless/victory-fork/pull/1312), Exopus; avatarmanz/VIctory-7900XTX `5f86fa3` |
| Licencia | [#1057](https://github.com/fxceless/victory-fork/pull/1057), nullexxx |
| Hilos de pipelines | [#955](https://github.com/fxceless/victory-fork/pull/955), LukaManstein |
| Watchdog del título | [#959](https://github.com/fxceless/victory-fork/pull/959), ruvenesh |
| Pruebas estables | [#963](https://github.com/fxceless/victory-fork/pull/963) y [#952](https://github.com/fxceless/victory-fork/pull/952), MamaLeVrai; [#978](https://github.com/fxceless/victory-fork/pull/978), avt0m4t48 |
| Arreglos de texturas de caras negras, quinta fusión del original | fxceless/victory-fork `e4245f3`, `93db653`, fxceless |
| Sombras en cascada exactas en 32 lanes | [#1002](https://github.com/fxceless/victory-fork/pull/1002), Bruno Magalhães |
| Destinos de render conservados al cortar una lista | [#1371](https://github.com/fxceless/victory-fork/pull/1371), spk-c |
| Más trabajo solo de rayos fuera, y contado | edsuuu/VI-BR-PORT `f1e5e45`, `24f8d2e` |
| Las mitades de los turnos `=alt`, intercambiadas a petición | [#1341](https://github.com/fxceless/victory-fork/pull/1341), Carver |
| Un recorte de la memoria de la propia capa | [#1286](https://github.com/fxceless/victory-fork/pull/1286), Vladimir |
| Ventanas sueltas reutilizadas; refrescos que pasan de largo los trozos sin escribir | [#1362](https://github.com/fxceless/victory-fork/pull/1362), Mat; [#1358](https://github.com/fxceless/victory-fork/pull/1358), TG0000 |
| Pasadas del montón encadenadas, texturas leídas por adelantado en hilos | [#1290](https://github.com/fxceless/victory-fork/pull/1290), Exopus |
| Teclas desde un fichero; huecos nulos contados por draw | este fork |
| Pase sobre el montón, más ligero | [#1414](https://github.com/fxceless/victory-fork/pull/1414), Kyle Soderlund |
| Arreglos del traductor de Vice-Tay y otros | TaynanGT/Vice-Tay `9bb7dbff`, `8fe3ccdd`, `17e3e2c8`, `037c5315`, `b7a99ac4`; [#682](https://github.com/fxceless/victory-fork/pull/682); #2031 (Flare) |
| Vida de los objetos | TaynanGT/Vice-Tay `2a267adb`; [#1377](https://github.com/fxceless/victory-fork/pull/1377) |
| De VI-BR-PORT, Belobug-AMD y pull requests del original | edsuuu/VI-BR-PORT; Belobug/VIctory-Belobug-AMD `86dc1097`, `4b82f6c2`, #118; [#1362](https://github.com/fxceless/victory-fork/pull/1362), Mat; [#1358](https://github.com/fxceless/victory-fork/pull/1358), TG0000; [#1290](https://github.com/fxceless/victory-fork/pull/1290), Exopus |
| Mapeos de página anotados, escritos en sala limpia | este fork (sala limpia, a partir de la idea de stellarr-studio/kelvaroq 0062) |
| Diagnósticos | este fork |

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

Puedes usar un mando XInput, y un DualSense de Sony (el mando de PlayStation 5) conectado por USB o Bluetooth se lee directamente como primer mando, sin controlador ni programa auxiliar (`VICTORY_SONY_PAD=0` lo deja en paz; sin vibración todavía; la lectura de sus informes está probada con tests, con el mando en la mano aún no). Sin mando, el teclado actúa como uno mientras la ventana tiene el foco: **WASD / IJKL** para los sticks, **Enter** para A, **Backspace** para B y **Esc / Tab** para Menu / View. [Controles completos y guía heredada](README.upstream.md#getting-started).

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
| FPS en el mundo | **De 6,2 a 6,7 fps** (frame mediano de 141 a 155 ms) en la primera sala en las sesiones guionizadas medidas, con interruptores puestos que por defecto van apagados; el código de `main` tal como está ahora midió 6,59 fps (frame mediano de 144 ms), y 6,87 fps (142 ms) con `VICTORY_TABLED_ALL=1`, una sesión de cada; 7,3 fps con uno más, al que un comprobador le encontró después un fallo y que no se recomienda. Bajones mientras carga una zona | Primera sala, perfil Series S, una maniobra programada de 280 s (la cámara gira y el personaje anda cada 20 s; unos 4500 draws por frame), con `VICTORY_SKIP_WC_BACK=1`, `VICTORY_COPY_PLAIN=1`, `VICTORY_TEXTURE_DROP_MIPS=1` y el conjunto de diez interruptores del original. Sin los diez: 5,99 fps. Con `VICTORY_INDIRECT_WINDOW=1` añadido: 7,31 fps, pero el comprobador del propio original encontró que ese interruptor lee un registro viejo, así que no se recomienda. Con el conjunto el límite es el procesador, no la tarjeta. Cada cifra es una sesión; los interruptores por su nombre, todas las cifras y sus condiciones están en [docs/measurements.es.md](docs/measurements.es.md). RTX 4060 Ti. |
| Estabilidad | 46 minutos en una sesión sin caídas | Series S, compilación `9b08008` con `VICTORY_SKIP_WC_BACK=1`, `VICTORY_COPY_PLAIN=1`, `VICTORY_AMBIENT=3` y `VICTORY_TEXTURE_DROP_MIPS=1`: 2812 s hasta el límite de la sesión, con sus procesos cerrados después por la suite de pruebas. El aviso de movimiento del juego se vio por primera vez a los 8 minutos; desde esa primera vista del mundo hasta el final, 38 minutos y 44 s, hubo frames registrados en todas las ventanas de cinco minutos, el título leyó las 25 pulsaciones programadas que movían la cámara y al personaje, y una persona jugó parte de la sesión y fue de la primera sala a un pasillo con otros personajes. La imagen solo se miró en momentos sueltos (esa primera vista y la captura de quien jugaba, 27 minutos después), así que no está demostrado que el mundo estuviera en pantalla ni bajo control todo el tiempo. Antes, ese mismo día: 924 s hasta el límite de la sesión, con `VICTORY_TEXTURE_ROOM_PERCENT=95`. Otra sesión de 30 minutos también llegó a su límite, pero su cierre no quedó registrado limpiamente y no la contamos. Esa misma tarde, siete sesiones de diez minutos sin nadie delante en Series S con `VICTORY_TEXTURE_DROP_MIPS=1` llegaron todas a su límite sin caída. En Series X dura menos en este equipo: se queda sin memoria física. |
| Imagen | Con el perfil para 8 GB (pone `VICTORY_AMBIENT=0.6`, un sustituto de la luz que el título saca de los rayos) la primera sala sale iluminada, con el personaje, la cama y las paredes; en las escenas del principio las caras de algunos personajes pasan a negro unos segundos, y algunas escenas pierden geometría o salen oscuras. Sin `VICTORY_AMBIENT` el mundo 3D sale casi todo oscuro. Las imágenes de carga, los subtítulos y los avisos se ven | Series S: el selector y las imágenes de carga se ven desde el #188 (medido aquí: dos planos en Series S, uno en Series X) y la sesión sigue hasta el juego. La imagen 3D sale casi negra y sin detalle, y las texturas se ven mal; los subtítulos y avisos del plano de interfaz se leen. Con `VICTORY_AMBIENT=0.6`, en una sesión, se vieron las paredes y una cama de una escena interior, mientras que un plano anterior de esa misma sesión seguía muy oscuro. En esa sala, un mismo encuadre con tres niveles puestos con el juego en marcha: a 3 las paredes, la cama, el pelo y el personaje se ven claramente más que a 0,6; a 12 paredes y personaje se queman. Así que la imagen oscura responde a esa luz, y el nivel que le va a una escena no es un número único. Lo que no arregla: artefactos en el pelo y el lado de una cara que siguió negro a 0,6 con las paredes iluminadas; a 3, un primer plano salió con los ojos blancos y brillantes y un pasillo demasiado claro. Quien jugó la sesión larga vio el brazo de un guardia dibujado con bloques de muchos colores y bocas mal dibujadas; la causa no está resuelta (véase Próximos pasos). El 27 de noviembre, en una veintena de sesiones con una captura por segundo con ese perfil, la sala mostró al personaje, la cama y las paredes en todas las que llegaron a ella, las bocas se movieron, y una cara de la cafetería del principio pasó a negro unos segundos en la mayoría; la causa aún no está encontrada ([medidas](docs/measurements.es.md)). Series X: sombras negras. |

Cómo medimos: los tiempos de frame que la capa anota cada unos cinco segundos, solo en el mundo y por escena. Las cifras salen de compilaciones de diagnóstico, que cuestan tiempo de frame. Un equipo y unas pocas sesiones.

## Medidas

**Para una tarjeta de 8 GB, tal como se ha medido aquí** (RTX 4060 Ti de 8 GB, i7-8700K, 32 GB de RAM): el perfil Series S, los diez interruptores de Estado, cinco más para esta tarjeta y `VICTORY_TABLED_ALL=1`, todos en las líneas de abajo. Con el código de `main` (compilación `f9f53eaa`, cuyo `crates/` es el de `main` desde la fusión del 27 de noviembre), la maniobra guionizada, una sesión: **6,87 fps**, frame mediano de 142 ms, percentil 95 de 169 ms; lo mismo sin `VICTORY_TABLED_ALL`, 6,59 fps y 144 ms. Hay que dejar `VICTORY_TEXTURE_DROP_MIPS=1`: sin él, miles de texturas esperan sitio en la tarjeta y objetos enteros salen negros. No pongas `VICTORY_DISPLAY_MODE=1920x1080` sin `VICTORY_OWN_UPSCALER=1` (el mundo sale negro), y ninguno de los dos da más sitio aquí.

```powershell
$env:VICTORY_SKIP_WC_BACK=1; $env:VICTORY_COPY_PLAIN=1; $env:VICTORY_TEXTURE_DROP_MIPS=1
$env:VICTORY_AMBIENT=0.6; $env:VICTORY_CARD_ROOM_FOR_TARGETS_MB=1536
$env:VICTORY_ASYNC_SUBMIT=1; $env:VICTORY_QUEUE2=1; $env:VICTORY_JOIN_PARTS=1; $env:VICTORY_BACK_OWN_VIEW=1
$env:VICTORY_INDIRECT_GATHER=1; $env:VICTORY_CARD_REHASH_RUNS=1; $env:VICTORY_CARD_HASH_LEN=1
$env:VICTORY_MERGE_GAP=64; $env:VICTORY_MERGE_RUNS=4096; $env:VICTORY_STAGING_POOL=0
$env:VICTORY_TABLED_ALL=1
.\victory.exe --console series-s <tu título>
```

Sesiones guionizadas en la primera sala, cada cifra una sesión. Antes de la fusión del 27 de noviembre el código de `main` midió 6,62 fps (frame mediano de 145 ms). De un frame de unos 148 ms, el hilo que envía espera entre 56 y 59 ms al candado de envío, casi todo a que la tarjeta termine trabajo cuyos bytes el título lee en el mismo frame: esa espera, y no solo el procesador, es lo que limita ahora el frame. En una sesión de 6,24 fps el hilo que envía trabajó 155 ms por frame, de ellos entre 60 y 70 ms de su propio tiempo de procesador y entre 40 y 53 ms esperando a la tarjeta. La primera medida dentro del juego, del 25 de noviembre (de 3,0 a 5,0 fps), las sesiones desde un SSD, las notas completas de la tabla de Estado y todas las cifras anteriores con sus condiciones están en [docs/measurements.es.md](docs/measurements.es.md).

## Próximos pasos

- **Primero la memoria.** El proceso tiene entre 16,1 y 16,8 GiB de memoria privada en la primera sala y hasta 19 GiB mientras carga el mundo, en un equipo de 32 GB; el pico de la carga es donde se cortan las sesiones. `VICTORY_WINDOW_KB=1024` solo ya se midió y no compensa (0,4 GiB menos en la sala, un 5,6 % menos de frames, el pico igual). Los siguientes son `VICTORY_WINDOW_TAILS` y `VICTORY_INPLACE`.
- **La imagen.** Oscura (el constructor de estructuras de trazado de rayos va apagado por defecto, y `VICTORY_AMBIENT` hace de sustituto de su luz), texturas sin sitio en 8 GB, bordes que bailan; y caras y brazos que pasan a negro, reproducidos el 26 de noviembre con el perfil por defecto, que no arreglan ni más luz ni hornear la piel con todas sus fuentes; los arreglos de caras negras del original están en `main` desde la noche del 26 de noviembre, y una cara siguió saliendo casi del todo negra con ellos en una de las dos sesiones que llegaron a la cafetería: no son toda la respuesta. La noche del 26 al 27 de noviembre: sin `VICTORY_TEXTURE_DROP_MIPS=1` miles de huecos de textura esperan sitio en la tarjeta de 8 GB y objetos enteros salen negros, así que ese interruptor hace falta; una pantalla de 1920x1080 no deja más sitio y cuesta fps; y una cara salió negra mientras 2611 texturas esperaban en la cola de subidas tras un cambio de plano, que es el siguiente sospechoso. Ver [docs/measurements.es.md](docs/measurements.es.md).
- **Los fps.** Lo que limita el frame es el hilo de envío de la propia capa, no la tarjeta: copias de memoria del título, el vaciado de refrescos, las preguntas al write-watch y los fallos de guarda. Una veintena de interruptores traídos de otros proyectos están en `main`, apagados, para medirlos de uno en uno.
- **Cargar y moverse.** Mientras el jugador se mueve o carga una zona los fps caen a 2 o 3 y menos: el refresco de texturas espera mientras el runtime remapea la memoria del título.
- **Estabilidad al cargar.** Una primera sesión de la fusión nueva, con la caché de shaders fría, la cerró el propio título a los 388 s tras un frame de 21,5 s esperando al remapeo del runtime; cuatro sesiones después llegaron a su límite de tiempo. `VICTORY_TEXTURE_NO_WAIT=1` (en `main`, apagado) evita que el refresco de texturas espere a eso, y es lo siguiente a medir.
- **En camino** desde TaynanGT/Vice-Tay: un filtro de NaN en las salidas de color, un muestreador de comparación que convertía el mínimo en máximo (sombras negras) y tres arreglos del traductor.

El relato completo, con sus cifras: [docs/whats-next.es.md](docs/whats-next.es.md).

## Colaborar y dar crédito

Son bienvenidos los cambios pequeños, con procedencia clara y una prueba reproducible. Incluye en los informes versión, hardware, controlador, ajustes y pasos. Revisa los logs antes de compartirlos y elimina rutas personales y datos privados. Usa casos sintéticos; consulta la [política de archivos](docs/asset-policy.md).

Gracias a **Marvisak y los colaboradores de VIctory**, **fxceless y los colaboradores de victory-fork**, **PiloteCode, gomeaw, Vax y PastiiJay**. Conservamos su historial y atribución. La [documentación heredada](README.upstream.md) mantiene los créditos y notas anteriores; sus cifras históricas no son mediciones de este fork.

La cabecera y el monograma VI son gráficos vectoriales originales creados para VIniVIdiVIci.

**GPL-3.0-or-later** · [Licencia](LICENSE) · [Código](https://github.com/KomanderDG88/VIniVIdiVIci) · [Incidencias](https://github.com/KomanderDG88/VIniVIdiVIci/issues)
