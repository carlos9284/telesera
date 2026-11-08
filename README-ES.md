# Telesera para LG webOS TV 3.4.3 - ARMv7 (prototipo experimental)

Preparado para un LG con:
- `uname -m`: `armv7l`
- kernel: `3.10.19-p.58.digya.k2lp.2`
- webOS: `3.4.3`
- glibc: `2.19`
- almacenamiento disponible: ~595 MiB

## Estado real

Este paquete **NO es una aplicacion lista para instalar**. Incluye una primera
adaptacion de compilacion ARMv7, la conversion ES5 de la interfaz y polyfills
para el navegador antiguo. NO se ha compilado el motor Rust en este entorno:
no hay Rust/Docker ni acceso a red para descargar dependencias. Tampoco ha
sido probado en el televisor. NO sustituye Chromium ni actualiza webOS.

La compilacion ARMv7 musl usa `armv7-unknown-linux-musleabihf`, que produce
codigo ARMv7 con hardware float y enlazado independiente de glibc. Aunque
webOS emplea ABI soft-float para sus bibliotecas nativas, un proceso
**estaticamente enlazado**, sin llamadas ABI a bibliotecas del sistema,
puede ejecutarse con hard-float en el mismo procesador. Esto necesita
verificacion real en el televisor; la comprobacion ELF rechaza `PT_INTERP`.

## Como usarlo con el codigo original

1. Hacer un fork de https://github.com/risingsunhq/telesera en tu GitHub.
2. Clonar tu fork en un ordenador y copiar el contenido de este ZIP sobre la
   raiz del repositorio. Copiar **la carpeta `.github`** incluida.
3. Hacer commit y push de `tools/webos3` y
   `.github/workflows/armv7-webos3-experimental.yml` en tu fork.
4. En GitHub abrir Actions > `Build Telesera webOS 3 ARMv7 (experimental)` >
   `Run workflow`. Requiere que Actions este habilitado.
5. Si el flujo termina correctamente, descargar el artifact
   `telesera-webos3-armv7-candidate` que contiene un archivo `.tar.gz`.

Si hay error de compilacion, **no instalar el binario**. Conserva el log para
investigar dependencias o el enlazador ARMv7. Las dependencias Rust se
resuelven en GitHub Actions y no se cambian las del proyecto original.

## Prueba segura: solo verificar que arranca el ejecutable

Extrae `telesera-engine` del `.tar.gz` en Windows. En PowerShell, desde la
carpeta que contiene ese archivo:

```powershell
ssh root@192.168.1.137 "mkdir -p /media/developer/telesera-test"
scp -O .\telesera-engine root@192.168.1.137:/media/developer/telesera-test/
ssh root@192.168.1.137 "chmod 700 /media/developer/telesera-test/telesera-engine && /media/developer/telesera-test/telesera-engine"
```

Sin `--config`, se espera una salida de uso similar a
`Usage: telesera-engine --config PATH`. Es intencional: comprueba que el kernel
puede cargar y ejecutar el binario, **no prueba Telegram**.

Un mensaje `not found`, `Illegal instruction`, `Segmentation fault` o
`Exec format error` indica que la prueba no se ha superado. No insistas
modificando bibliotecas de `/lib` ni particiones del televisor.

## Interfaz: lo que se ha adaptado

`prepare-ui.js` transforma `ui/app.js`, `ui/i18n.js` y el lanzador webOS a
JavaScript ES5 usando TypeScript, y genera `dist/webos3/ui` y
`dist/webos3/webos` SIN sobrescribir los originales.

`compat-webos3.js` agrega implementaciones limitadas de `fetch` (sobre XHR),
`AbortController`, `URLSearchParams`, `Array.from`, `.find()`, `.includes()`,
`padStart`, `NodeList.forEach` y funciones del DOM antiguas. Se anade un CSS
de respaldo para las partes de la interfaz que usan CSS Grid y variables CSS.

Hay riesgo de incompatibilidades aun no detectadas en Chromium 38, en el
compositor y en la reproduccion multimedia. Probar la interfaz por separado
antes de autenticar una cuenta real de Telegram.

## Siguientes pasos tras la primera prueba

- Probar un motor con configuracion privada y credenciales propias de Telegram,
  enlazado solo a `127.0.0.1` y puerto 3190, sin exponer credenciales.
- Verificar la interfaz webOS 3.4.3 real: acceso, listas, teclado, mando y foco.
- Crear un instalador especificamente verificado para ARMv7 y hacer pruebas
  con cuenta Telegram de prueba antes de usar la cuenta principal.

Este kit deliberadamente no incluye instalador automatico ni archivos con
sesiones/tokens Telegram. No borrar ni escribir `rootfs`, `kernel` o
`tvservice`. No desactivar medidas de seguridad de la TV.

## Referencia

Codigo original (MIT): https://github.com/risingsunhq/telesera
Modificaciones experimentales independientes para esta prueba.
