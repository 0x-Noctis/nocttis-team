// M5-010: penormal respons untuk provider yang tidak mematuhi kontrak OpenAI chat/completions.
//
// Router lokal yang dipakai benchmark (a) SELALU membalas SSE walau `stream` tidak diminta dan (b) tidak mengirim penutup
// `data: [DONE]`. Klien Noctis mengikuti kontrak OpenAI: respons non-streaming harus JSON, dan stream tanpa [DONE]
// diperlakukan sebagai koneksi putus (provider_unavailable). Proxy penghitung di mvp-run.cjs memakai fungsi ini supaya
// benchmark tetap bisa berjalan TANPA mengubah klien produksi. Isi token tidak diubah, jadi hitungan usage tetap asli.

/** Baris `data: {...}` JSON dari teks SSE (abaikan [DONE] dan baris non-JSON). */
function sseEvents(text) {
  const events = [];
  for (const line of text.split('\n')) {
    if (!line.startsWith('data:')) continue;
    const payload = line.slice(5).trim();
    if (!payload || payload === '[DONE]') continue;
    try { events.push(JSON.parse(payload)); } catch { /* baris rusak dilewati; usage tetap dihitung dari yang valid */ }
  }
  return events;
}

/** Gabungkan potongan SSE menjadi satu objek `chat.completion` (konten, tool_calls per index, finish_reason, usage). */
function aggregate(events) {
  const first = events[0] ?? {};
  let content = '';
  let finish = null;
  let usage;
  const calls = new Map();
  for (const event of events) {
    if (event.usage) usage = event.usage;
    const choice = event.choices?.[0];
    if (!choice) continue;
    if (choice.finish_reason) finish = choice.finish_reason;
    const delta = choice.delta ?? {};
    if (typeof delta.content === 'string') content += delta.content;
    for (const call of delta.tool_calls ?? []) {
      const entry = calls.get(call.index ?? 0) ?? { id: '', type: 'function', function: { name: '', arguments: '' } };
      if (call.id) entry.id = call.id;
      if (call.function?.name) entry.function.name += call.function.name;
      if (call.function?.arguments) entry.function.arguments += call.function.arguments;
      calls.set(call.index ?? 0, entry);
    }
  }
  const message = { role: 'assistant', content: content || (calls.size ? null : '') };
  if (calls.size) message.tool_calls = [...calls.entries()].sort(([a], [b]) => a - b).map(([, call]) => call);
  return {
    id: first.id ?? 'chatcmpl-normalized',
    object: 'chat.completion',
    created: first.created ?? 0,
    model: first.model ?? '',
    choices: [{ index: 0, message, finish_reason: finish ?? 'stop' }],
    ...(usage ? { usage } : {})
  };
}

/**
 * Hasil yang dikirim ke klien. `wantsStream` = body request berisi `stream: true`.
 * Respons yang sudah sesuai kontrak (JSON untuk non-stream, SSE ber-[DONE] untuk stream, atau error) tidak diubah.
 */
function normalize(wantsStream, contentType, bodyText) {
  const isSse = /text\/event-stream/i.test(contentType ?? '') || bodyText.trimStart().startsWith('data:');
  if (!isSse) return { contentType, body: bodyText };
  if (wantsStream) {
    const done = /data:\s*\[DONE\]\s*$/.test(bodyText.trimEnd());
    return { contentType: 'text/event-stream', body: done ? bodyText : `${bodyText.trimEnd()}\n\ndata: [DONE]\n\n` };
  }
  return { contentType: 'application/json', body: JSON.stringify(aggregate(sseEvents(bodyText))) };
}

module.exports = { normalize, aggregate, sseEvents };
