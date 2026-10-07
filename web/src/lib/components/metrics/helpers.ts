import type {
  ComponentName,
  ComponentState,
  CostView,
  OperationsView,
  ProviderReadinessView,
  UsageView
} from './types';

export const COMPONENT_LABELS: Record<ComponentName, string> = {
  database: 'Database',
  migrations: 'Migrations',
  workers: 'Workers',
  providers: 'Model providers'
};

// Status SELALU memuat ikon + kata, tidak hanya warna.
const STATE_PRESENTATION: Record<ComponentState, { label: string; icon: string }> = {
  ok: { label: 'OK', icon: '✓' },
  degraded: { label: 'Degraded', icon: '!' },
  down: { label: 'Down', icon: '×' }
};

export interface ComponentPresentation {
  name: ComponentName;
  title: string;
  state: ComponentState;
  label: string;
  icon: string;
  explanation: string;
}

export function describeComponent(name: ComponentName, state: ComponentState, view: Pick<OperationsView, 'stale_attempts'>): ComponentPresentation {
  const explanations: Record<ComponentName, Record<ComponentState, string>> = {
    database: {
      ok: 'The database responds.',
      degraded: 'The database is slow or partially available.',
      down: 'The database is unreachable. The API cannot read or write any state.'
    },
    migrations: {
      ok: 'The schema matches this build.',
      degraded: 'The schema needs attention.',
      down: 'The schema does not match this build (pending, failed, or unknown migration). Do not run workers until fixed.'
    },
    workers: {
      ok: 'No active attempt has missed its heartbeat.',
      degraded: `${view.stale_attempts ?? 'Some'} active attempt(s) stopped sending heartbeats; recovery will reclaim them.`,
      down: 'Worker state could not be read.'
    },
    providers: {
      ok: 'At least one model has verified tool support.',
      degraded: 'No model has verified tool support, so tasks cannot start. Register a model and run its tools probe.',
      down: 'Provider data could not be read.'
    }
  };
  return { name, title: COMPONENT_LABELS[name], state, ...STATE_PRESENTATION[state], explanation: explanations[name][state] };
}

export function overallStatus(view: Pick<OperationsView, 'ready' | 'components'>): { label: string; icon: string; detail: string } {
  if (!view.ready) return { label: 'Not ready', icon: '×', detail: 'The service cannot serve requests safely.' };
  const degraded = Object.values(view.components).some((state) => state !== 'ok');
  return degraded
    ? { label: 'Ready, with warnings', icon: '!', detail: 'The service is up, but a component needs attention.' }
    : { label: 'Ready', icon: '✓', detail: 'All components are healthy.' };
}

export const totalTokens = (usage: Pick<UsageView, 'input_tokens' | 'output_tokens'>) => usage.input_tokens + usage.output_tokens;

// Estimasi bagian dari total (0..100); total nol berarti tidak ada yang bisa diestimasi.
export function estimatedPercent(usage: UsageView): number {
  const total = totalTokens(usage);
  return total > 0 ? Math.min(100, Math.round((usage.estimated_tokens / total) * 100)) : 0;
}

// Biaya dari provider hanya ada bila dicatat; mata uang tidak dicatat, jadi disebut "cost units".
export function formatCost(cost: CostView | undefined): string {
  if (!cost || !cost.tracked) return 'Not tracked (no provider reported a cost)';
  const partial = cost.priced_rows < cost.total_rows ? ` — covers ${cost.priced_rows} of ${cost.total_rows} requests` : '';
  return `${(cost.micros / 1_000_000).toFixed(4)} cost units (currency not recorded)${partial}`;
}

export const formatNumber = (value: number) => value.toLocaleString('en-US');

export function providerIssues(provider: ProviderReadinessView): string[] {
  const issues: string[] = [];
  if (!provider.secret_configured) issues.push('API key environment variable is not set on the server.');
  if (provider.models.length === 0) issues.push('No models are registered for this provider.');
  else if (!provider.models.some((model) => model.tools === 'supported')) issues.push('No model has verified tool support; run the tools probe.');
  return issues;
}

export const humanize = (key: string) => key.replace(/_/g, ' ').replace(/^./, (first) => first.toUpperCase());

export function formatConfigValue(value: unknown): string {
  if (typeof value === 'boolean') return value ? 'Yes' : 'No';
  if (value === null || value === undefined) return 'Not set';
  if (Array.isArray(value)) return value.length ? value.map(String).join(', ') : 'None';
  return String(value);
}
