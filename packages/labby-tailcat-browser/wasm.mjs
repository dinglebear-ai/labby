let loading;
let loadedManifest;
const maximumBytes = 48 * 1024 * 1024;
const hex = bytes => [...new Uint8Array(bytes)].map(b => b.toString(16).padStart(2, '0')).join('');

async function readArtifact(response) {
  if (!response.body || Number(response.headers.get('content-length')) > maximumBytes) {
    await response.body?.cancel();
    throw Error('Tailcat asset budget exceeded');
  }
  const reader = response.body.getReader();
  const chunks = [];
  let length = 0;
  try {
    for (;;) {
      const {value, done} = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > maximumBytes) throw Error('Tailcat asset budget exceeded');
      chunks.push(value);
    }
  } catch (error) { await reader.cancel(); throw error; }
  finally { reader.releaseLock(); }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  return bytes;
}

/** Lazy load immutable self-hosted assets with independently supplied hashes. */
export function loadTailcat({baseURL, wasmSha256, runtimeSha256}) {
  let base;
  try {
    base = new URL(baseURL, location.href);
    if (base.origin !== location.origin || !base.pathname.endsWith('/') || base.search || base.hash || base.username || base.password) {
      throw Error('Invalid Tailcat asset root');
    }
    if (!/^[a-f0-9]{64}$/.test(wasmSha256) || !/^[a-f0-9]{64}$/.test(runtimeSha256)) throw Error('Tailcat asset checksums required');
  } catch (error) { return Promise.reject(error); }
  const manifest = JSON.stringify([base.href, wasmSha256, runtimeSha256]);
  if (loading) {
    if (loadedManifest !== manifest) return Promise.reject(Error('Tailcat asset manifest changed'));
    return loading;
  }
  loadedManifest = manifest;
  loading = (async () => {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 30000);
    let script;
    let bootStarted = false;
    try {
      const response = await fetch(new URL('tailcat.wasm', base), {signal: controller.signal, redirect: 'error'});
      if (!response.ok) throw Error('Tailcat WASM unavailable');
      const bytes = await readArtifact(response);
      if (hex(await crypto.subtle.digest('SHA-256', bytes)) !== wasmSha256) throw Error('Tailcat WASM checksum mismatch');
      script = document.createElement('script');
      script.src = new URL('wasm_exec.js', base).href;
      const raw = runtimeSha256.match(/../g).map(n => String.fromCharCode(parseInt(n, 16))).join('');
      script.integrity = 'sha256-' + btoa(raw);
      script.crossOrigin = 'anonymous';
      await new Promise((resolve, reject) => {
        const abort = () => reject(Error('Tailcat boot deadline exceeded'));
        controller.signal.addEventListener('abort', abort, {once: true});
        const settle = callback => { controller.signal.removeEventListener('abort', abort); callback(); };
        script.onload = () => settle(resolve);
        script.onerror = () => settle(() => reject(Error('Tailcat runtime unavailable')));
        document.head.append(script);
        if (controller.signal.aborted) abort();
      });
      const go = new globalThis.Go();
      const {instance} = await WebAssembly.instantiate(bytes, go.importObject);
      if (controller.signal.aborted) throw Error('Tailcat boot deadline exceeded');
      bootStarted = true;
      await new Promise((resolve, reject) => {
        const abort = () => reject(Error('Tailcat boot deadline exceeded'));
        controller.signal.addEventListener('abort', abort, {once: true});
        globalThis.onTailcatReady = () => { controller.signal.removeEventListener('abort', abort); resolve(); };
        go.run(instance).then(() => reject(Error('Tailcat runtime exited')), () => reject(Error('Tailcat runtime failed')));
      });
      if (typeof globalThis.tailcatIdentity !== 'function' || typeof globalThis.tailcatSession !== 'function') throw Error('Tailcat exports unavailable');
      return {identity: globalThis.tailcatIdentity, createSession: globalThis.tailcatSession};
    } catch (error) {
      script?.remove();
      // A running Go instance cannot safely restart in the same page.
      if (!bootStarted) { loading = undefined; loadedManifest = undefined; }
      throw error;
    } finally { clearTimeout(timer); }
  })();
  return loading;
}
