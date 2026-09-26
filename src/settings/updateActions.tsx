import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Icon } from '../shell/icons';
import { showResultToast } from '../components/toast';
import { checkForUpdate, installUpdate, type UpdateCheckResult } from './api';
import { openExternalUrl } from '../providers/api';

/** GitHub 回退时引导到的发布页（与「查看发布」一致）。 */
const RELEASES_URL = 'https://github.com/dailyoozoo/helm-desktop/releases/latest';

/** 检查更新的行内状态条反馈（对齐原型 settings.html 的 cm-about-status-bar）。 */
export interface UpdateFeedback {
  kind: 'checking' | 'latest' | 'available' | 'error';
  message: string;
}

/** 真实更新链路（P2-1）：检查 → 展示新版本 → 下载安装（进度来自 update-progress 事件）。 */
export function UpdateActions({
  feedConfigured,
  onFeedback,
}: {
  feedConfigured: boolean;
  onFeedback?: (feedback: UpdateFeedback | null) => void;
}) {
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [available, setAvailable] = useState<UpdateCheckResult | null>(null);
  const [progress, setProgress] = useState<{ downloaded: number; total: number | null } | null>(
    null,
  );

  useEffect(() => {
    if (!installing) return;
    let unlisten: (() => void) | null = null;
    let active = true;
    void listen<{ downloaded?: number; total?: number | null; finished?: boolean }>(
      'update-progress',
      (event) => {
        if (!active) return;
        if (event.payload.finished) {
          setProgress(null);
          return;
        }
        setProgress({
          downloaded: event.payload.downloaded ?? 0,
          total: event.payload.total ?? null,
        });
      },
    ).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [installing]);

  const handleCheck = async () => {
    setChecking(true);
    setAvailable(null);
    onFeedback?.({ kind: 'checking', message: '正在检查更新…' });
    try {
      const result = await checkForUpdate();
      if (result.available) {
        setAvailable(result);
        // feed 源可应用内安装；GitHub 回退只提示版本并引导到发布页下载
        const message =
          result.source === 'github'
            ? 'GitHub 上发现新版本 v' + (result.version ?? '')
            : '发现新版本 v' + (result.version ?? '');
        onFeedback?.({ kind: 'available', message });
        showResultToast(message);
      } else {
        onFeedback?.({
          kind: 'latest',
          message: '当前已是最新版本（v' + result.currentVersion + '）',
        });
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      onFeedback?.({ kind: 'error', message: '检查更新失败：' + message });
      showResultToast('检查更新失败：' + message);
    } finally {
      setChecking(false);
    }
  };

  const handleOpenRelease = async () => {
    if (!available) return;
    try {
      await openExternalUrl(available.releaseUrl || RELEASES_URL);
    } catch (error) {
      showResultToast(
        '打开发布页失败：' + (error instanceof Error ? error.message : String(error)),
      );
    }
  };

  const handleInstall = async () => {
    setInstalling(true);
    setProgress(null);
    try {
      await installUpdate();
      // 成功路径应用会自动重启，走不到这里
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      onFeedback?.({ kind: 'error', message: '安装更新失败：' + message });
      showResultToast('安装更新失败：' + message);
      setInstalling(false);
      setProgress(null);
    }
  };

  const progressText =
    progress && progress.total
      ? '下载中 ' + Math.min(100, Math.round((progress.downloaded / progress.total) * 100)) + '%'
      : '下载中…';

  return (
    <div className="row gap-sm" style={{ alignItems: 'center' }}>
      <button
        className="btn btn--subtle btn--sm"
        type="button"
        disabled={checking || installing}
        title={
          feedConfigured
            ? undefined
            : '使用内置官方发布源（GitHub latest.json），可在应用内一键下载安装；如需自定义，在设置 → 通用里填写发布源地址'
        }
        onClick={handleCheck}
      >
        <Icon name="refresh" /> {checking ? '检查中…' : '检查更新'}
      </button>
      {available?.available ? (
        available.source === 'github' ? (
          <button
            className="btn btn--primary btn--sm"
            type="button"
            onClick={() => void handleOpenRelease()}
          >
            <Icon name="gitbranch" /> 前往下载 v{available.version}
          </button>
        ) : (
          <button
            className="btn btn--primary btn--sm"
            type="button"
            disabled={installing}
            onClick={handleInstall}
          >
            <Icon name="down" /> {installing ? progressText : '下载并安装 v' + available.version}
          </button>
        )
      ) : null}
      {available?.notes ? <span className="faint">{available.notes}</span> : null}
    </div>
  );
}
