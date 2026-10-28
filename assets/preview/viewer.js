// Only this bundled code runs; document text is decoded as data, never evaluated.
(async () => {
  const image = document.getElementById('graphic');
  const viewport = document.getElementById('viewport');
  const status = document.getElementById('status');
  const controls = [...document.querySelectorAll('button')];
  const report = (state) => window.ipc?.postMessage(`tshell:${state}`);
  let scale = 1;
  let fitted = true;
  function zoom(value) {
    scale = Math.max(0.1, Math.min(8, value));
    image.style.width = `${image.naturalWidth * scale}px`;
    image.style.height = `${image.naturalHeight * scale}px`;
    document.getElementById('scale').textContent = `${Math.round(scale * 100)}%`;
  }
  function fit() {
    fitted = true;
    zoom(Math.min((viewport.clientWidth - 32) / image.naturalWidth,
      (viewport.clientHeight - 32) / image.naturalHeight));
  }
  function fail(error) {
    status.hidden = false;
    status.textContent = `${status.dataset.error}${error?.message ? `\n${error.message.slice(0, 1500)}` : ''}`;
    viewport.setAttribute('aria-busy', 'false');
    image.hidden = true;
    controls.forEach(button => { button.disabled = true; });
    report('error');
  }
  try {
    const bytes = Uint8Array.from(atob(document.getElementById('source').dataset.source), c => c.charCodeAt(0));
    let svg = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    if (document.documentElement.dataset.renderer === 'mermaid') {
      const mermaid = window.tshellMermaid;
      mermaid.initialize({
        startOnLoad: false, securityLevel: 'strict', theme: 'default',
        maxTextSize: 32768, maxEdges: 500, suppressErrorRendering: true,
        htmlLabels: false, flowchart: { htmlLabels: false },
        secure: ['secure', 'securityLevel', 'startOnLoad', 'maxTextSize', 'maxEdges', 'suppressErrorRendering', 'htmlLabels'],
      });
      svg = (await mermaid.render('tshell-diagram', svg)).svg;
    }
    // SVG in an image context cannot execute scripts, navigate, or fetch subresources.
    image.src = `data:image/svg+xml;base64,${btoa(Array.from(new TextEncoder().encode(svg), b => String.fromCharCode(b)).join(''))}`;
    await image.decode();
    if (!image.naturalWidth || !image.naturalHeight) throw new Error();
    image.hidden = false;
    status.hidden = true;
    viewport.setAttribute('aria-busy', 'false');
    controls.forEach(button => { button.disabled = false; });
    document.getElementById('out').onclick = () => { fitted = false; zoom(scale / 1.25); };
    document.getElementById('in').onclick = () => { fitted = false; zoom(scale * 1.25); };
    document.getElementById('actual').onclick = () => { fitted = false; zoom(1); };
    document.getElementById('fit').onclick = fit;
    viewport.addEventListener('keydown', event => {
      if (event.ctrlKey || event.altKey || event.metaKey) return;
      if (event.key === '+' || event.key === '=') { fitted = false; zoom(scale * 1.25); }
      else if (event.key === '-') { fitted = false; zoom(scale / 1.25); }
      else if (event.key === '0') fit();
      else return;
      event.preventDefault();
    });
    new ResizeObserver(() => { if (fitted) fit(); }).observe(viewport);
    fit();
    report('ready');
  } catch (error) { fail(error); }
})();
