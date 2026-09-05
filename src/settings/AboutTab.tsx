import { useCallback, useEffect, useMemo, useState } from 'react';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { Icon } from '../shell/icons';
import { EngineBrand } from '../shell/EngineBrand';
import { showResultToast } from '../components/toast';
import { Dialog } from '../components/Dialog';
import {
  exportDiagnosticsBundle,
  getLogDirInfo,
  getPlatformInfo,
  importHistory,
  listImportableHistories,
  type ImportableHistoryEntry,
  type ImportableHistoryScan,
  type LogDirInfo,
  type PlatformInfo,
} from './api';
import { openPathInSystem } from '../engine/transport';
import { UpdateActions } from './updateActions';
import { SetupWizardModal, type PageIdLike } from './SetupWizardModal';
import type { AppSettings } from './types';

const RELEASES_URL = 'https://github.com/dailyoozoo/helm-desktop/releases/';

/**
 * 设置页「关于」（S8）：版本/平台信息、检查更新、数据与日志入口。
 * 所有动作都接真实命令：get_platform_info / get_log_dir_info /
 * export_diagnostics_bundle / list_importable_histories / import_history。
 */
export function AboutTab({
  settings,
  update,
  onNavigate,
}: {
  settings: AppSettings;
  update: (updater: (prev: AppSettings) => AppSettings) => void;
  onNavigate?: (page: PageIdLike) => void;
}) {
  const [platform, setPlatform] = useState<PlatformInfo | null>(null);
  const [logDir, setLogDir] = useState<LogDirInfo | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [wizardOpen, setWizardOpen] = useState(false);
  const [feedConfigured, setFeedConfigured] = useState(
    () =>
      settings.general.updateFeedUrl.trim().length > 0 ||
      settings.general.pricingFeedUrls.length > 0,
  );

  useEffect(() => {
    setFeedConfigured(
      settings.general.updateFeedUrl.trim().length > 0 ||
        settings.general.pricingFeedUrls.length > 0,
    );
  }, [settings.general.updateFeedUrl, settings.general.pricingFeedUrls]);

  const refreshLogDir = useCallback(() => {
    getLogDirInfo()
      .then(setLogDir)
      .catch(() => setLogDir(null));
  }, []);

  useEffect(() => {
    let active = true;
    getPlatformInfo()
      .then((info) => {
        if (active) setPlatform(info);
      })
      .catch(() => {
        if (active) setPlatform(null);
      });
    refreshLogDir();
    return () => {
      active = false;
    };
  }, [refreshLogDir]);

  const platformText = useMemo(() => {
    if (!platform) return '';
    return [
      'OS: ' + platform.osName + (platform.osVersion ? ' ' + platform.osVersion : ''),
      'Arch: ' + platform.arch,
      'Helm: ' + platform.appVersion,
      'Tauri: ' + platform.tauriVersion,
      'WebView: ' + platform.webviewVersion,
    ].join('\n');
  }, [platform]);

  const copyPlatform = async () => {
    if (!platformText) return;
    try {
      await navigator.clipboard.writeText(platformText);
      showResultToast('平台信息已复制');
    } catch (error) {
      showResultToast('复制失败：' + (error instanceof Error ? error.message : String(error)));
    }
  };

  const openLogs = async () => {
    try {
      // 先取真实日志目录（不存在则创建），再交给系统文件管理器打开。
      const info = await getLogDirInfo();
      setLogDir(info);
      await openPathInSystem(info.path);
      showResultToast('已打开日志文件夹');
    } catch (error) {
      showResultToast(
        '打开日志文件夹失败：' + (error instanceof Error ? error.message : String(error)),
      );
    }
  };

  const exportDiagnostics = async () => {
    try {
      const result = await exportDiagnosticsBundle();
      if (!result) return; // 用户取消
      showResultToast(
        '诊断包已完成脱敏并导出（' +
          Math.max(1, Math.round(result.bytes / 1024)) +
          ' KB）：' +
          result.path,
      );
      refreshLogDir();
    } catch (error) {
      showResultToast(
        '导出诊断包失败：' + (error instanceof Error ? error.message : String(error)),
      );
    }
  };

  const pickImportFile = async () => {
    try {
      const selected = await openFileDialog({
        title: '选择 JSONL 历史记录',
        multiple: false,
        filters: [{ name: '对话记录 (JSONL)', extensions: ['jsonl'] }],
      });
      if (typeof selected !== 'string') return;
      setImportBusyPath(selected);
      try {
        const result = await importHistory({ sourcePath: selected, engine: 'auto' });
        showResultToast(
          '导入完成：' +
            result.title +
            '（' +
            result.importedMessages +
            ' 条消息，跳过 ' +
            result.skippedLines +
            ' 行）',
        );
        void markImported(selected);
      } catch (error) {
        showResultToast('导入失败：' + (error instanceof Error ? error.message : String(error)));
      } finally {
        setImportBusyPath(null);
      }
    } catch (error) {
      showResultToast('选择文件失败：' + (error instanceof Error ? error.message : String(error)));
    }
  };

  const [importBusyPath, setImportBusyPath] = useState<string | null>(null);
  const [importedPaths, setImportedPaths] = useState<string[]>([]);
  const markImported = (path: string) =>
    setImportedPaths((current) => (current.includes(path) ? current : [...current, path]));

  return (
    <section>
      <div className="cm-about-hero">
        <div className="cm-about-hero__brand">
          <span className="cm-mark cm-mark--large">
            <Icon name="helm" />
          </span>
          <div>
            <h2>Helm</h2>
            <p>
              版本 {platform?.appVersion ?? '…'} · Tauri {platform?.tauriVersion ?? '2'} 桌面客户端
            </p>
          </div>
        </div>
        <div className="row gap-sm">
          <a
            className="cm-action cm-action--quiet"
            href={RELEASES_URL}
            target="_blank"
            rel="noopener noreferrer"
          >
            <Icon name="gitbranch" /> 查看发布
          </a>
          <UpdateActions feedConfigured={feedConfigured} />
        </div>
      </div>

      <div className="cm-section">
        <div className="cm-section__head">
          <div>
            <h2>
              <Icon name="cpu" /> 平台信息
            </h2>
            <p>用于反馈问题时核对运行环境。</p>
          </div>
          <button className="cm-action" type="button" onClick={() => void copyPlatform()}>
            <Icon name="copy" /> 复制
          </button>
        </div>
        {platform ? (
          <div className="cm-detail-card">
            <div className="cm-option-row">
              <div className="cm-option-row__main">
                <b>操作系统</b>
                <small>
                  {platform.osName}
                  {platform.osVersion ? ' · 内核 ' + platform.osVersion : ''} · {platform.arch}
                </small>
              </div>
            </div>
            <div className="cm-option-row">
              <div className="cm-option-row__main">
                <b>应用版本</b>
                <small className="mono">
                  Helm {platform.appVersion} · Tauri {platform.tauriVersion} · WebView{' '}
                  {platform.webviewVersion}
                </small>
              </div>
            </div>
          </div>
        ) : (
          <div className="empty" aria-live="polite">
            正在读取平台信息…
          </div>
        )}
      </div>

      <div className="cm-section">
        <div className="cm-section__head">
          <div>
            <h2>
              <Icon name="layers" /> 数据与日志
            </h2>
            <p>诊断包导出前会移除密钥、完整环境变量和敏感内容。</p>
          </div>
        </div>
        <div className="cm-about-tiles">
          <button className="cm-about-tile" type="button" onClick={() => setImportOpen(true)}>
            <span className="cm-about-tile__icon">
              <Icon name="history" />
            </span>
            <span className="cm-about-tile__body">
              <b>导入历史对话</b>
              <small>从 Claude Code / Codex 或 JSONL 文件导入已有对话</small>
            </span>
            <span className="cm-about-tile__arrow">
              <Icon name="right" />
            </span>
          </button>
          <button className="cm-about-tile" type="button" onClick={() => setWizardOpen(true)}>
            <span className="cm-about-tile__icon">
              <Icon name="rocket" />
            </span>
            <span className="cm-about-tile__body">
              <b>进入安装向导</b>
              <small>检查 Agent CLI 与运行依赖</small>
            </span>
            <span className="cm-about-tile__arrow">
              <Icon name="right" />
            </span>
          </button>
          <button className="cm-about-tile" type="button" onClick={() => void openLogs()}>
            <span className="cm-about-tile__icon">
              <Icon name="folderopen" />
            </span>
            <span className="cm-about-tile__body">
              <b>打开日志文件夹</b>
              <small>查看运行日志与错误记录</small>
            </span>
            <span className="cm-about-tile__arrow">
              <Icon name="right" />
            </span>
          </button>
          <button className="cm-about-tile" type="button" onClick={() => void exportDiagnostics()}>
            <span className="cm-about-tile__icon">
              <Icon name="archive" />
            </span>
            <span className="cm-about-tile__body">
              <b>导出诊断包</b>
              <small>自动脱敏后打包便于反馈</small>
            </span>
            <span className="cm-about-tile__arrow">
              <Icon name="right" />
            </span>
          </button>
        </div>
        {logDir?.lastDiagnosticsExport ? (
          <p className="faint" style={{ marginTop: 10 }}>
            上次导出：{logDir.lastDiagnosticsExport.exportedAt} →{' '}
            <span className="mono">{logDir.lastDiagnosticsExport.path}</span>
          </p>
        ) : null}
      </div>

      {importOpen ? (
        <ImportHistoryModal
          importedPaths={importedPaths}
          busyPath={importBusyPath}
          onClose={() => setImportOpen(false)}
          onPickFile={() => void pickImportFile()}
        />
      ) : null}
      {wizardOpen ? (
        <SetupWizardModal
          update={update}
          onNavigate={onNavigate}
          onClose={() => setWizardOpen(false)}
        />
      ) : null}
    </section>
  );
}

// ─── 导入历史对话 ───────────────────────────────────────────────────────────

type ImportStep = 'select-agent' | 'select-conv';

function ImportHistoryModal({
  importedPaths,
  busyPath,
  onClose,
  onPickFile,
}: {
  importedPaths: string[];
  busyPath: string | null;
  onClose: () => void;
  onPickFile: () => void;
}) {
  const [step, setStep] = useState<ImportStep>('select-agent');
  const [engine, setEngine] = useState<'claude-code' | 'codex'>('claude-code');
  const [scan, setScan] = useState<ImportableHistoryScan | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [results, setResults] = useState<{ path: string; ok: boolean; detail: string }[]>([]);
  const [importing, setImporting] = useState(false);

  useEffect(() => {
    if (step !== 'select-conv') return;
    let active = true;
    setLoading(true);
    setError(null);
    listImportableHistories(engine)
      .then((next) => {
        if (active) setScan(next);
      })
      .catch((err: unknown) => {
        if (active) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [engine, step]);

  const toggle = (path: string) =>
    setSelected((current) =>
      current.includes(path) ? current.filter((item) => item !== path) : [...current, path],
    );

  const confirmImport = async () => {
    setImporting(true);
    try {
      for (const path of selected) {
        try {
          const result = await importHistory({ sourcePath: path, engine });
          setResults((current) => [
            ...current.filter((item) => item.path !== path),
            {
              path,
              ok: true,
              detail:
                '已导入 ' +
                result.importedMessages +
                ' 条消息（跳过 ' +
                result.skippedLines +
                ' 行）→ ' +
                result.title,
            },
          ]);
        } catch (err) {
          setResults((current) => [
            ...current.filter((item) => item.path !== path),
            { path, ok: false, detail: err instanceof Error ? err.message : String(err) },
          ]);
        }
      }
    } finally {
      setImporting(false);
    }
  };

  return (
    <Dialog title="导入历史对话" onClose={onClose}>
      <p className="muted" style={{ marginBottom: 12 }}>
        从本机 Agent 记录导入已有对话，转为 Helm 任务继续工作。内容只写入本地数据库，不会上传。
      </p>

      {step === 'select-agent' ? (
        <div className="cm-import-agent-list">
          <button
            className="cm-import-agent"
            type="button"
            onClick={() => {
              setEngine('claude-code');
              setSelected([]);
              setResults([]);
              setStep('select-conv');
            }}
          >
            <span className="cm-import-agent__icon">
              <EngineBrand engine="claude-code" size={16} />
            </span>
            <span className="cm-import-agent__body">
              <b>Claude Code</b>
              <small>扫描本机 Claude Code 项目记录（JSONL）</small>
            </span>
            <span className="cm-import-agent__arrow">
              <Icon name="right" />
            </span>
          </button>
          <button
            className="cm-import-agent"
            type="button"
            onClick={() => {
              setEngine('codex');
              setSelected([]);
              setResults([]);
              setStep('select-conv');
            }}
          >
            <span className="cm-import-agent__icon">
              <EngineBrand engine="codex" size={16} />
            </span>
            <span className="cm-import-agent__body">
              <b>Codex</b>
              <small>扫描本机 Codex 会话 rollout（JSONL）</small>
            </span>
            <span className="cm-import-agent__arrow">
              <Icon name="right" />
            </span>
          </button>
          <button className="cm-import-agent" type="button" onClick={onPickFile}>
            <span className="cm-import-agent__icon">
              <Icon name="file" />
            </span>
            <span className="cm-import-agent__body">
              <b>从文件导入</b>
              <small>
                {busyPath
                  ? '正在导入：' + busyPath
                  : '支持 Claude Code / Codex 形状的 JSONL 对话记录'}
              </small>
            </span>
            <span className="cm-import-agent__arrow">
              <Icon name="right" />
            </span>
          </button>
        </div>
      ) : (
        <div className="cm-import-step is-active">
          <div className="cm-import-breadcrumb">
            <button type="button" onClick={() => setStep('select-agent')}>
              <Icon name="left" /> {engine === 'codex' ? 'Codex' : 'Claude Code'}
            </button>
            <Icon name="right" />
            <span>选择对话</span>
          </div>

          {loading ? <div className="empty">正在扫描本机记录…</div> : null}
          {error ? (
            <div className="settings-inline-error" role="alert">
              <span>{error}</span>
            </div>
          ) : null}

          {scan && scan.entries.length === 0 ? (
            <div className="empty">没有发现可导入的记录（目录存在但无匹配 JSONL）。</div>
          ) : null}

          {scan && scan.entries.length ? (
            <div className="cm-import-conv-list">
              {scan.entries.map((entry) => (
                <ImportRow
                  key={entry.path}
                  entry={entry}
                  checked={selected.includes(entry.path)}
                  imported={importedPaths.includes(entry.path)}
                  result={results.find((item) => item.path === entry.path)}
                  onToggle={() => toggle(entry.path)}
                />
              ))}
            </div>
          ) : null}

          {scan && (scan.skippedTooLarge > 0 || scan.skippedUnparsable > 0) ? (
            <p className="faint" style={{ marginTop: 8 }}>
              已跳过 {scan.skippedTooLarge} 个超大文件、{scan.skippedUnparsable} 个无法解析的文件；
              共发现 {scan.totalFound} 个记录文件。
            </p>
          ) : null}

          {results.length ? (
            <div className="st-import-results" role="status">
              {results.map((result) => (
                <div key={result.path} className={result.ok ? 'is-ok' : 'is-fail'}>
                  <Icon name={result.ok ? 'checkc' : 'alert'} />
                  <span>{result.detail}</span>
                </div>
              ))}
            </div>
          ) : null}

          <div className="cm-import-foot">
            <span className="cm-import-foot__count">
              {selected.length ? '已选择 ' + selected.length + ' 个对话' : '未选择'}
            </span>
            <button className="btn btn--subtle btn--sm" type="button" onClick={onClose}>
              关闭
            </button>
            <button
              className="btn btn--primary btn--sm"
              type="button"
              disabled={!selected.length || importing}
              onClick={() => void confirmImport()}
            >
              {importing ? '导入中…' : '导入选中对话'}
            </button>
          </div>
        </div>
      )}
    </Dialog>
  );
}

function ImportRow({
  entry,
  checked,
  imported,
  result,
  onToggle,
}: {
  entry: ImportableHistoryEntry;
  checked: boolean;
  imported: boolean;
  result?: { ok: boolean; detail: string };
  onToggle: () => void;
}) {
  const modified = new Date(entry.modifiedAtMs).toLocaleDateString('zh-CN');
  return (
    <label className={'cm-import-conv' + (checked ? ' is-selected' : '')}>
      <input
        type="checkbox"
        className="sr-only"
        checked={checked}
        onChange={onToggle}
        disabled={imported}
      />
      <span className="cm-import-conv__check">
        <Icon name="checkc" />
      </span>
      <span className="cm-import-conv__main">
        <b>{entry.firstMessagePreview || entry.fileName}</b>
        <small>
          {entry.messageCount} 条消息
          {entry.cwd ? ' · ' + entry.cwd : ''}
          {entry.model ? ' · ' + entry.model : ''}
        </small>
        {imported ? <small className="is-ok">已导入过（本次会话内）</small> : null}
        {result && !result.ok ? <small className="is-fail">{result.detail}</small> : null}
      </span>
      <span className="cm-import-conv__meta">{modified}</span>
    </label>
  );
}
