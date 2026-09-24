// Pages serves static files without configurable Content-Encoding headers.
// Restore the original rustdoc document at the requested URL so its relative
// links, fragments, search scripts and source browser keep their normal base.
(async () => {
  const content = document.querySelector('meta[name="oer-api-content"]');
  const message = document.getElementById('api-loading');
  try {
    if (!('DecompressionStream' in globalThis)) {
      throw new Error('This API viewer needs a browser with gzip decompression support.');
    }
    const response = await fetch(new URL(content.content, document.baseURI));
    if (!response.ok) throw new Error(`The documentation page could not be loaded (HTTP ${response.status}).`);
    const stream = response.body.pipeThrough(new DecompressionStream('gzip'));
    const html = await new Response(stream).text();
    document.open();
    document.write(html);
    document.close();
  } catch (error) {
    message.setAttribute('role', 'alert');
    message.textContent = `${error.message} You can retry or download the compressed page below.`;
  }
})();
