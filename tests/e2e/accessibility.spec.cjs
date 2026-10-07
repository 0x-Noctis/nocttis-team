// M5-004: audit aksesibilitas dan responsif otomatis di seluruh halaman utama, desktop dan ponsel.
// Tidak memakai axe-core (dependensi baru di luar lingkup); pemeriksaan ditulis langsung di halaman:
// bahasa/judul/landmark/heading, nama aksesibel, label form, id ganda, tabindex positif, kontras warna terhitung,
// indikator fokus, reduced-motion, overflow horizontal, dan ukuran target sentuh (WCAG 2.2 AA 2.5.8, 24px).
const { expect, test } = require('../../web/node_modules/@playwright/test');
const { randomUUID } = require('node:crypto');
const { spawnSync } = require('node:child_process');

const POSTGRES = 'noctis-agent-3-e2e-postgres';
const suffix = Date.now().toString(36);
const project = randomUUID();
const run = randomUUID();
const taskId = `a11y-needs-${suffix}`;

function sql(statement) {
  const result = spawnSync(
    'docker',
    ['exec', '-i', POSTGRES, 'psql', '-U', 'ai_team', '-d', 'ai_team', '-v', 'ON_ERROR_STOP=1', '-c', statement],
    { encoding: 'utf8' }
  );
  if (result.status !== 0) throw new Error(`psql failed: ${result.stderr}`);
}

test.beforeAll(() => {
  // Run dengan task berbagai status supaya semua varian lencana status ikut diperiksa. Tanpa model terdaftar,
  // scheduler tidak menjalankan apa pun.
  const insert = (id, status) =>
    `INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts) VALUES ('${id}','${run}','worker','Task ${status}','Objective ${status}','${status}','["a.txt"]','["works"]','["cat a.txt"]',1000,1000,2)`;
  sql(
    [
      `INSERT INTO projects (id,name,repository_path) VALUES ('${project}','A11y project','/tmp/a11y-${suffix}')`,
      `INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ('${run}','${project}','a11y','RUNNING',100000)`,
      insert(`a11y-ready-${suffix}`, 'READY'),
      insert(`a11y-done-${suffix}`, 'DONE'),
      insert(taskId, 'NEEDS_HUMAN'),
      insert(`a11y-cancel-${suffix}`, 'CANCELLED'),
      insert(`a11y-draft-${suffix}`, 'DRAFT')
    ].join(';\n')
  );
});

const routes = () => [
  ['home', '/'],
  ['providers', '/providers'],
  ['projects', '/projects'],
  ['project detail', `/projects/${project}`],
  ['run detail', `/runs/${run}`],
  ['tasks', '/tasks'],
  ['task detail', `/tasks/${taskId}`],
  ['approvals', '/approvals'],
  ['operations', '/operations'],
  ['settings', '/settings']
];

/** Dijalankan di dalam halaman; mengembalikan daftar pelanggaran sebagai string. */
function auditInPage({ mobile }) {
  const violations = [];
  const describe = (element) => {
    const text = (element.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 40);
    return `<${element.tagName.toLowerCase()}${element.id ? `#${element.id}` : ''}${element.className && typeof element.className === 'string' ? `.${element.className.split(' ')[0]}` : ''}> "${text}"`;
  };
  const visible = (element) => {
    const style = getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    return style.visibility !== 'hidden' && style.display !== 'none' && rect.width > 0 && rect.height > 0;
  };

  if (!document.documentElement.lang) violations.push('html tanpa atribut lang');
  if (!document.title.trim()) violations.push('halaman tanpa <title>');
  if (document.querySelectorAll('main').length !== 1) violations.push(`jumlah <main> = ${document.querySelectorAll('main').length}`);
  const h1 = [...document.querySelectorAll('h1')].filter(visible);
  if (h1.length !== 1) violations.push(`jumlah <h1> terlihat = ${h1.length}`);

  const ids = new Map();
  for (const element of document.querySelectorAll('[id]')) ids.set(element.id, (ids.get(element.id) ?? 0) + 1);
  for (const [id, count] of ids) if (count > 1) violations.push(`id ganda: ${id} (${count}x)`);
  for (const element of document.querySelectorAll('[tabindex]')) {
    if (Number(element.getAttribute('tabindex')) > 0) violations.push(`tabindex positif: ${describe(element)}`);
  }

  const labelledText = (element) => {
    const ids = element.getAttribute('aria-labelledby');
    if (!ids) return '';
    return ids.split(/\s+/).map((id) => document.getElementById(id)?.textContent ?? '').join(' ').trim();
  };
  const accessibleName = (element) =>
    (element.getAttribute('aria-label') || labelledText(element) || '').trim() ||
    (element.tagName === 'INPUT' && ['button', 'submit', 'reset'].includes(element.type) ? element.value : '').trim() ||
    (element.labels ? [...element.labels].map((label) => label.textContent).join(' ').trim() : '') ||
    (element.textContent ?? '').trim() ||
    (element.getAttribute('title') ?? '').trim() ||
    (element.querySelector('img[alt]')?.getAttribute('alt') ?? '').trim();

  const controls = [...document.querySelectorAll('a[href], button, input:not([type=hidden]), select, textarea, [role=button], [role=link], [role=tab], summary')].filter(visible);
  for (const element of controls) {
    if (!accessibleName(element)) violations.push(`tanpa nama aksesibel: ${describe(element)}`);
  }
  for (const element of document.querySelectorAll('img')) {
    if (!element.hasAttribute('alt')) violations.push(`<img> tanpa alt: ${describe(element)}`);
  }

  // Kontras warna teks terhitung (WCAG 1.4.3: 4.5:1, teks besar 3:1).
  const parse = (value) => {
    let match = /^rgba?\(([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:[,\s/]+([\d.]+%?))?\)$/.exec(value);
    if (match) {
      const alpha = match[4] === undefined ? 1 : match[4].endsWith('%') ? parseFloat(match[4]) / 100 : parseFloat(match[4]);
      return [Number(match[1]), Number(match[2]), Number(match[3]), alpha];
    }
    match = /^color\(srgb ([\d.]+) ([\d.]+) ([\d.]+)(?: \/ ([\d.]+))?\)$/.exec(value);
    if (match) return [match[1] * 255, match[2] * 255, match[3] * 255, match[4] === undefined ? 1 : Number(match[4])];
    return null;
  };
  const over = (top, below) => {
    const alpha = top[3];
    return [0, 1, 2].map((i) => top[i] * alpha + below[i] * (1 - alpha)).concat(1);
  };
  const luminance = ([r, g, b]) => {
    const channel = (value) => {
      const v = value / 255;
      return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
  };
  const background = (element) => {
    const layers = [];
    for (let node = element; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (style.backgroundImage !== 'none') return null; // gradien/gambar: tidak bisa dihitung andal
      const color = parse(style.backgroundColor);
      if (color && color[3] > 0) {
        layers.push(color);
        if (color[3] === 1) break;
      }
    }
    return layers.reverse().reduce((below, layer) => over(layer, below), [255, 255, 255, 1]);
  };
  const seen = new Set();
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const element = node.parentElement;
    if (!element || seen.has(element) || !node.textContent.trim() || !visible(element)) continue;
    // Kontrol yang dinonaktifkan dikecualikan dari persyaratan kontras (WCAG 1.4.3).
    if (element.closest(':disabled, [aria-disabled=true]')) continue;
    if (['SCRIPT', 'STYLE', 'NOSCRIPT'].includes(element.tagName)) continue;
    seen.add(element);
    const style = getComputedStyle(element);
    const foreground = parse(style.color);
    const base = background(element);
    if (!foreground || !base) continue;
    const text = over([foreground[0], foreground[1], foreground[2], foreground[3] * Number(style.opacity)], base);
    const [light, dark] = [luminance(text), luminance(base)].sort((a, b) => b - a);
    const ratio = (light + 0.05) / (dark + 0.05);
    const size = parseFloat(style.fontSize);
    const large = size >= 24 || (size >= 18.66 && Number(style.fontWeight) >= 700);
    if (ratio < (large ? 3 : 4.5)) violations.push(`kontras ${ratio.toFixed(2)}:1 < ${large ? 3 : 4.5}: ${describe(element)}`);
  }

  if (document.documentElement.scrollWidth > window.innerWidth + 1) {
    violations.push(`overflow horizontal: scrollWidth ${document.documentElement.scrollWidth} > ${window.innerWidth}`);
  }
  if (mobile) {
    for (const element of controls) {
      const rect = element.getBoundingClientRect();
      const inline = getComputedStyle(element).display === 'inline' && element.tagName === 'A';
      if (!inline && (rect.width < 24 || rect.height < 24)) violations.push(`target sentuh ${Math.round(rect.width)}x${Math.round(rect.height)} < 24: ${describe(element)}`);
    }
  }
  return violations;
}

for (const [viewportName, viewport] of [['desktop', { width: 1280, height: 900 }], ['mobile', { width: 375, height: 700 }]]) {
  test.describe(`a11y ${viewportName}`, () => {
    test.use({ viewport });
    for (const [label, path] of routes()) {
      test(`${label}: struktur, nama, kontras, ${viewportName === 'mobile' ? 'target sentuh dan overflow' : 'overflow'}`, async ({ page }) => {
        await page.goto(path);
        await page.waitForLoadState('networkidle');
        const violations = await page.evaluate(auditInPage, { mobile: viewportName === 'mobile' });
        expect(violations, violations.join('\n')).toEqual([]);
      });
    }
  });
}

// ---- Keyboard: fokus terlihat di setiap perhentian Tab, tanpa jebakan fokus ----
test.describe('a11y keyboard', () => {
  for (const [label, path] of routes()) {
    test(`${label}: setiap perhentian Tab punya indikator fokus dan fokus tidak terjebak`, async ({ page }) => {
      await page.goto(path);
      await page.waitForLoadState('networkidle');
      const stops = new Set();
      const problems = [];
      for (let index = 0; index < 80; index += 1) {
        await page.keyboard.press('Tab');
        const state = await page.evaluate(() => {
          const element = document.activeElement;
          if (!element || element === document.body) return null;
          const style = getComputedStyle(element);
          const outline = style.outlineStyle !== 'none' && parseFloat(style.outlineWidth) > 0;
          const shadow = style.boxShadow !== 'none';
          const name = `${element.tagName.toLowerCase()} "${(element.textContent ?? '').trim().slice(0, 30)}"`;
          return { key: `${name}#${[...document.querySelectorAll(element.tagName)].indexOf(element)}`, name, visibleFocus: outline || shadow };
        });
        if (state === null) break; // fokus keluar dari dokumen: siklus selesai, tidak terjebak
        if (stops.has(state.key)) break; // kembali ke perhentian pertama
        stops.add(state.key);
        if (!state.visibleFocus) problems.push(`tanpa indikator fokus: ${state.name}`);
      }
      expect(stops.size, 'halaman harus punya perhentian Tab').toBeGreaterThan(0);
      expect(stops.size, 'jumlah perhentian Tab tidak wajar (jebakan fokus?)').toBeLessThan(80);
      expect(problems, problems.join('\n')).toEqual([]);
    });
  }

  test('reduced motion: preferensi pengguna mematikan transisi dan animasi', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto('/providers');
    const result = await page.evaluate(() => {
      const probe = document.createElement('div');
      probe.style.cssText = 'transition: opacity 2s; animation: probe-spin 2s infinite;';
      document.body.append(probe);
      const style = getComputedStyle(probe);
      const durations = { transition: style.transitionDuration, animation: style.animationDuration };
      probe.remove();
      return { matches: matchMedia('(prefers-reduced-motion: reduce)').matches, durations, running: document.getAnimations().length };
    });
    expect(result.matches).toBe(true);
    expect(parseFloat(result.durations.transition)).toBeLessThan(0.01);
    expect(parseFloat(result.durations.animation)).toBeLessThan(0.01);
    expect(result.running).toBe(0);
  });

  test('status tidak hanya warna: setiap status task dan lencana run tampil sebagai teks', async ({ page }) => {
    // Daftar task: setiap status seed muncul sebagai kata, bukan sekadar warna/ikon.
    await page.goto('/tasks');
    await expect(page.getByText('Loading tasks…')).toHaveCount(0);
    for (const status of ['READY', 'DONE', 'NEEDS_HUMAN', 'CANCELLED', 'DRAFT']) {
      await expect(page.locator('ul.tasks li', { hasText: `Task ${status}` }).first()).toContainText(status);
    }
    // Halaman run: semua elemen berkelas badge/status membawa teks yang bermakna.
    await page.goto(`/runs/${run}`);
    await expect(page.getByText('Loading run…')).toHaveCount(0);
    const badges = await page.$$eval('[class*="badge"]', (elements) =>
      elements.map((element) => ({ text: (element.textContent ?? '').replace(/[^\p{L}\p{N}]/gu, ''), name: element.getAttribute('aria-label') ?? '' }))
    );
    expect(badges.length, 'halaman run harus punya lencana status').toBeGreaterThan(0);
    for (const badge of badges) expect(badge.text.length, `lencana tanpa teks: ${JSON.stringify(badge)}`).toBeGreaterThan(1);
  });
});

// ---- Alur utama selesai hanya dengan keyboard (tanpa klik): provider, project -> run ----
test.describe('a11y keyboard-only flows', () => {
  const key = () => ({ 'Idempotency-Key': randomUUID() });

  /** Ketik ke field berlabel hanya dengan fokus + keyboard. */
  async function typeInto(page, label, text) {
    const field = page.getByLabel(label, { exact: false }).first();
    await field.focus();
    await page.keyboard.press('Control+A');
    await page.keyboard.type(text);
  }
  async function activate(page, role, name) {
    const control = page.getByRole(role, { name });
    await control.focus();
    await expect(control).toBeFocused();
    await page.keyboard.press('Enter');
  }

  test('provider: tambah lewat form hanya dengan keyboard', async ({ page, request }) => {
    const id = `kbd-provider-${suffix}`;
    await page.goto('/providers');
    await expect(page.getByText('Memuat providers…')).toHaveCount(0);
    await activate(page, 'button', 'Tambah provider');
    await typeInto(page, 'Provider ID', id);
    await typeInto(page, 'Base URL', 'http://127.0.0.1:7411/v1');
    await typeInto(page, 'API key environment variable', 'PRIMARY_API_KEY');
    await activate(page, 'button', 'Save provider');
    await expect(page.getByRole('button', { name: new RegExp(id) })).toBeVisible();
    await request.delete(`/api/v1/providers/${id}`, { headers: key() });
  });

  test('project dan run: dibuat hanya dengan keyboard', async ({ page }) => {
    const { mkdtempSync, writeFileSync, rmSync } = require('node:fs');
    const { tmpdir } = require('node:os');
    const { join } = require('node:path');
    const repository = mkdtempSync(join(tmpdir(), 'noctis-kbd-'));
    spawnSync('git', ['init', '--initial-branch=main', repository]);
    spawnSync('git', ['-C', repository, 'config', 'user.name', 'Noctis E2E']);
    spawnSync('git', ['-C', repository, 'config', 'user.email', 'noctis@example.invalid']);
    writeFileSync(join(repository, 'a.txt'), 'x\n');
    spawnSync('git', ['-C', repository, 'add', '.']);
    spawnSync('git', ['-C', repository, 'commit', '-m', 'fixture']);
    try {
      await page.goto('/projects');
      await expect(page.getByText(/Loading projects/)).toHaveCount(0);
      await typeInto(page, 'Name', `Keyboard project ${suffix}`);
      await typeInto(page, 'Repository path', repository);
      await activate(page, 'button', 'Save project');
      await expect(page).toHaveURL(/\/projects\/[0-9a-f-]{36}$/);
      await expect(page.getByText('Loading project…')).toHaveCount(0);

      await typeInto(page, 'Objective', 'Keyboard-only run');
      await typeInto(page, 'Acceptance criteria', 'It works');
      await typeInto(page, 'Token budget', '50000');
      await activate(page, 'button', 'Create run');
      await expect(page).toHaveURL(/\/runs\/[0-9a-f-]{36}$/);
      await expect(page.getByRole('heading', { name: 'Run details' })).toBeVisible();
    } finally {
      rmSync(repository, { recursive: true, force: true });
    }
  });
});
