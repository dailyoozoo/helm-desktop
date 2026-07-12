import { useEffect, useState } from 'react';
import type { SessionState } from '../../engine/useSession';
import { Icon } from '../../shell/icons';
import { activityPresentationParts } from '../activityViewModel';

export function ActivityRow({ state }: { state: SessionState }) {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [state.turnActivity?.since, state.turnActivity?.stage]);

  const presentation = activityPresentationParts(state, now);
  if (!presentation) return null;

  return (
    <div className="item">
      <div className="item__gut">
        <div className="ava-bot">
          <Icon name="bot" />
        </div>
      </div>
      <div className="item__main">
        <div className="working activity-row">
          <span className="pulse" aria-hidden="true">
            <i />
            <i />
            <i />
          </span>
          <span className="activity-row__text">
            <span role="status" aria-live="polite">
              {presentation.label}
            </span>
            {presentation.elapsedText ? (
              <span aria-live="off">{presentation.elapsedText}</span>
            ) : null}
          </span>
        </div>
      </div>
    </div>
  );
}
