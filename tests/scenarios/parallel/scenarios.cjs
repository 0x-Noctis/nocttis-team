// Skenario deterministik untuk E2E paralel (M4-010). Fake provider mengenali task dari penanda
// `[ptask:<id>;kunci=nilai;...]` di objective task, sehingga balasannya tidak bergantung pada urutan request
// (beberapa worker berjalan bersamaan, jadi penghitung global seperti di vertical smoke tidak bisa dipakai).
//
// Kunci perilaku: file/from/to (patch satu baris), delay (ms, menahan balasan worker pertama supaya task
// benar-benar tumpang tindih), rdelay (ms, menahan balasan reviewer), reject=1 (review pertama meminta
// perubahan), tokens (total token per balasan, untuk menguji budget).

const MARKER = /\[ptask:([^\]]+)\]/;

const seen = new Map(); // task id -> { reviews }

function parseMarker(request) {
  const found = MARKER.exec(JSON.stringify(request));
  if (!found) return null;
  const [id, ...pairs] = found[1].split(';');
  const options = Object.fromEntries(pairs.map((pair) => pair.split('=')));
  return { id, options };
}

const isParallelRequest = (request) => parseMarker(request) !== null;

const patchFor = ({ file, from = 'base', to }) =>
  `diff --git a/${file} b/${file}\n--- a/${file}\n+++ b/${file}\n@@ -1 +1 @@\n-${from}\n+${to}\n`;

// Mengembalikan { message, delayMs, tokens } untuk request worker/reviewer task paralel.
function reply(request) {
  const { id, options } = parseMarker(request);
  const state = seen.get(id) ?? { reviews: 0 };
  seen.set(id, state);
  const tokens = Number(options.tokens ?? 20);
  const isReview = !request.tools?.length;

  if (isReview) {
    const delayMs = Number(options.rdelay ?? 0);
    state.reviews += 1;
    if (options.reject === '1' && state.reviews === 1) {
      const finding = { severity: 'medium', code: 'REWORK_NEEDED', message: 'Please redo the change.' };
      return { message: { role: 'assistant', content: JSON.stringify({ decision: 'changes_requested', findings: [finding] }) }, delayMs, tokens };
    }
    return { message: { role: 'assistant', content: JSON.stringify({ decision: 'approved' }) }, delayMs, tokens };
  }

  const toolResults = (request.messages ?? []).filter(({ role }) => role === 'tool').length;
  if (toolResults === 0) {
    const patch = patchFor({ file: options.file, from: options.from, to: options.to ?? `${id}-done` });
    return {
      message: {
        role: 'assistant',
        content: null,
        tool_calls: [{ id: `patch-${id}`, type: 'function', function: { name: 'apply_patch', arguments: JSON.stringify({ patch }) } }]
      },
      delayMs: Number(options.delay ?? 0),
      tokens
    };
  }
  return { message: { role: 'assistant', content: JSON.stringify({ summary: `${id} patched.`, status: 'self_check' }) }, delayMs: 0, tokens };
}

// Isi repository fixture: satu baris per file supaya patch satu baris selalu valid.
const FIXTURE_FILES = {
  'a.txt': 'base\n',
  'b.txt': 'base\n',
  'c.txt': 'base\n',
  'd.txt': 'base\n',
  'src/a.txt': 'base\n',
  'src/b.txt': 'base\n',
  'ui.txt': 'base\n',
  'core.txt': 'v1\n'
};

// Penanda objective untuk task `id` dengan opsi perilaku.
const objective = (id, options) =>
  `Change ${options.file} only. [ptask:${[id, ...Object.entries(options).map(([key, value]) => `${key}=${value}`)].join(';')}]`;

module.exports = { FIXTURE_FILES, isParallelRequest, objective, reply };
