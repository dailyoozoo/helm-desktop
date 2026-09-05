import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Icon } from '../shell/icons';
import { showResultToast } from '../components/toast';
import { checkForUpdate, installUpdate, type UpdateCheckResult } from './api';

/** 真实更新链路（P2-1）：检查 → 展示新版本 → 下载安装（进度来自 update-progress 事件）。 */
export function UpdateActions({ feedConfigured }: { feedConfigured: boolean }) {
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
    try {
      const result = await checkForUpdate();
      if (result.available) {
        setAvailable(result);
        showResultToast('发现新版本 v' + (result.version ?? ''));
      } else {
        showResultToast('当前已是最新版本（v' + result.currentVersion + '）');
      }
    } catch (error) {
      showResultToast('检查更新失败：' + (error instanceof Error ? error.message : String(error)));
    } finally {
      setChecking(false);
    }
  };

  const handleInstall = async () => {
    setInstalling(true);
    setProgress(null);
    try {
      await installUpdate();
      // 成功路径应用会自动重启，走不到这里
    } catch (error) {
      showResultToast('安装更新失败：' + (error instanceof Error ? error.message : String(error)));
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
        disabled={!feedConfigured || checking || installing}
        title={feedConfigured ? undefined : '先填写更新发布源或价格目录镜像'}
        onClick={handleCheck}
      >
        <Icon name="refresh" /> {checking ? '检查中…' : '检查更新'}
      </button>
      {available?.available ? (
        <button
          className="btn btn--primary btn--sm"
          type="button"
          disabled={installing}
          onClick={handleInstall}
        >
          <Icon name="down" /> {installing ? progressText : '下载并安装 v' + available.version}
        </button>
      ) : null}
      {available?.notes ? <span className="faint">{available.notes}</span> : null}
    </div>
  );
}
