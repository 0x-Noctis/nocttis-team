// Pembantu bersama antar spec E2E (M5-009): pembersihan model sisa dan pengulangan request idempoten.

/** Ulangi `fn` bila gagal karena koneksi (mis. soket keep-alive yang baru ditutup). Hanya untuk request idempoten. */
async function retrying(fn, attempts = 3) {
  let lastError;
  for (let index = 0; index < attempts; index += 1) {
    try {
      return await fn();
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 300));
    }
  }
  throw lastError;
}

/**
 * Hapus provider yang masih memiliki model `e2e-model` (sisa spec sebelumnya yang gagal). Id model unik, jadi satu sisa
 * saja membuat setiap spec berikutnya gagal mendaftar; pembersihan ini memutus rantai kegagalan beruntun.
 */
async function clearLeftoverModel(request, key) {
  const existing = await retrying(() => request.get('/api/v1/models/e2e-model'));
  if (!existing.ok()) return;
  const { provider_id: providerId } = await existing.json();
  await retrying(() => request.delete(`/api/v1/providers/${providerId}`, { headers: key() }));
}

module.exports = { retrying, clearLeftoverModel };
