import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Search, Trash2 } from 'lucide-react';
import { Sheet } from './Sheet';
import { useToastStore } from '../store/toast';
import * as api from '../api/commands';
import type { LogEntry } from '../types';

const LEVELS = ['ALL', 'ERROR', 'WARN', 'INFO'] as const;
type Level = (typeof LEVELS)[number];

function formatTs(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

export function LogPanel({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [level, setLevel] = useState<Level>('ALL');
  const [query, setQuery] = useState('');
  const listRef = useRef<HTMLDivElement>(null);

  const refresh = useCallback(async () => {
    try {
      setLogs(await api.getLogs(level === 'ALL' ? undefined : level));
      setError(null);
    } catch (e) {
      setError(api.extractTauriError(e));
    }
  }, [level]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  // Follow new lines only while the user is at the bottom.
  useEffect(() => {
    const el = listRef.current;
    if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 80) el.scrollTop = el.scrollHeight;
  }, [logs]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q
      ? logs.filter(
          (l) => l.message.toLowerCase().includes(q) || l.target.toLowerCase().includes(q),
        )
      : logs;
  }, [logs, query]);

  const clear = async () => {
    try {
      await api.clearLogs();
      setLogs([]);
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    }
  };

  const copy = async () => {
    const text = filtered
      .map((l) => `${new Date(l.ts).toISOString()} ${l.level} ${l.target}: ${l.message}`)
      .join('\n');
    try {
      await navigator.clipboard.writeText(text);
      showToast('success', t('logs.copied'));
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    }
  };

  return (
    <Sheet
      size="wide"
      title={t('logs.title')}
      description={t('logs.subtitle')}
      onClose={onClose}
      footer={
        <>
          <span className="hint grow">{t('logs.count', { count: filtered.length })}</span>
          <button
            type="button"
            className="btn"
            onClick={() => void copy()}
            disabled={!filtered.length}
          >
            {t('logs.copy')}
          </button>
          <button type="button" className="btn primary" data-modal-cancel onClick={onClose}>
            {t('common.done')}
          </button>
        </>
      }
    >
      <div className="log-tools">
        <div className="segmented" role="radiogroup" aria-label={t('logs.level')}>
          {LEVELS.map((lv) => (
            <button
              key={lv}
              type="button"
              role="radio"
              aria-checked={level === lv}
              onClick={() => setLevel(lv)}
            >
              {lv === 'ALL' ? t('logs.all') : lv}
            </button>
          ))}
        </div>
        <div className="search">
          <Search size={13} aria-hidden={true} />
          <input
            type="search"
            placeholder={t('logs.search')}
            aria-label={t('logs.search')}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <button
          type="button"
          className="icon-btn danger"
          onClick={() => void clear()}
          title={t('logs.clear')}
          aria-label={t('logs.clear')}
        >
          <Trash2 size={15} aria-hidden={true} />
        </button>
      </div>
      <div className="log-list" ref={listRef} role="log" aria-label={t('logs.title')}>
        {error && !logs.length ? (
          <div className="log-empty" role="alert">
            {t('logs.load_error')}
          </div>
        ) : filtered.length === 0 ? (
          <div className="log-empty">{t('logs.empty')}</div>
        ) : (
          filtered.map((entry, index) => (
            <div key={`${entry.ts}-${index}`} className={`log-row ${entry.level.toLowerCase()}`}>
              <span className="ts">{formatTs(entry.ts)}</span>
              <span className="lv">{entry.level}</span>
              <span className="msg">
                <span className="target">{entry.target.replace(/^falcon_dm_lib::/, '')} </span>
                {entry.message}
              </span>
            </div>
          ))
        )}
      </div>
    </Sheet>
  );
}
