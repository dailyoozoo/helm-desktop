import { Icon } from '../../shell/icons';

export function WorkingIndicator() {
  return (
    <div className="item">
      <div className="item__gut">
        <div className="ava-bot">
          <Icon name="bot" />
        </div>
      </div>
      <div className="item__main">
        <div className="working">
          <span className="pulse">
            <i />
            <i />
            <i />
          </span>
          <span>Helm 正在思考…</span>
        </div>
      </div>
    </div>
  );
}
