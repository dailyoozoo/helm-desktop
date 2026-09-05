import { Icon } from '../shell/icons';

export const FULL_ACCESS_CONFIRM_COPY =
  'Helm 将跳过审批，Agent 可直接修改文件和执行命令。仅当前任务生效，应用重启后自动回落。';

/** 两页共用的「全部放开」页内确认卡（原型 wsconfirm）。 */
export function FullAccessConfirm({
  titleId,
  onCancel,
  onConfirm,
}: {
  titleId: string;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div className="wsconfirm" role="dialog" aria-modal="true" aria-labelledby={titleId}>
      <div className="wsconfirm__card">
        <div className="wsconfirm__t" id={titleId}>
          <span className="cm-confirm-ic">
            <Icon name="alert" />
          </span>
          开启「全部放开」？
        </div>
        <p>{FULL_ACCESS_CONFIRM_COPY}</p>
        <div className="wsconfirm__acts">
          <button className="cm-action" type="button" onClick={onCancel}>
            取消
          </button>
          <button className="cm-action cm-action--danger" type="button" onClick={onConfirm}>
            开启全部放开
          </button>
        </div>
      </div>
    </div>
  );
}
