/** 主侧栏「处理完成」徽标的本机已查看记录（2026-08-25 用户规格）。
 *  只存“最近一次打开该任务的时间”，纯 UI 状态，不进数据库；
 *  代价（用户已接受）：重装/换电脑后记录丢失，各任务会重新标一轮未看。
 *  判定语义：任务 updatedAt 晚于 seenAt 即“有没看过的新结果”——
 *  因此看完后又跑完一轮，会自然重新算作未看。 */
const SEEN_KEY = 'helm.railSeen.v1';
const SEEN_MAX_ENTRIES = 200;

type SeenMap = Record<string, number>;

function loadMap(): SeenMap {
  try {
    const raw = window.localStorage.getItem(SEEN_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as SeenMap;
    if (!parsed || typeof parsed !== 'object') return {};
    const out: SeenMap = {};
    for (const [id, ts] of Object.entries(parsed)) {
      if (typeof id === 'string' && id && typeof ts === 'number' && Number.isFinite(ts) && ts > 0) {
        out[id] = ts;
      }
    }
    return out;
  } catch {
    return {};
  }
}

function saveMap(map: SeenMap): void {
  try {
    // 有界：只保留最近打开的 200 个任务，删除的会话记录自然淘汰
    const entries = Object.entries(map)
      .sort((a, b) => b[1] - a[1])
      .slice(0, SEEN_MAX_ENTRIES);
    window.localStorage.setItem(SEEN_KEY, JSON.stringify(Object.fromEntries(entries)));
  } catch {
    // 隐私模式等写入失败可接受：徽标最多多亮一轮
  }
}

/** 记录“用户此刻打开了该任务”。 */
export function markRailTaskSeen(sessionId: string): void {
  if (!sessionId) return;
  const map = loadMap();
  map[sessionId] = Date.now();
  saveMap(map);
}

/** 该任务最近一次被打开的时间（epoch ms）；从未打开过返回 null。 */
export function railTaskSeenAt(sessionId: string): number | null {
  if (!sessionId) return null;
  const ts = loadMap()[sessionId];
  return typeof ts === 'number' ? ts : null;
}
