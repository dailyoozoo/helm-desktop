export type SaveState = 'idle' | 'pending' | 'saving' | 'saved' | 'error';

export class LatestSerialSaver<T> {
  private timer: ReturnType<typeof setTimeout> | null = null;
  private pending: T | undefined;
  private chain: Promise<void> = Promise.resolve();

  constructor(
    private readonly save: (value: T) => Promise<void>,
    private readonly delayMs: number,
    private readonly onState?: (state: SaveState) => void,
    private readonly onError?: (error: unknown) => void,
  ) {}

  schedule(value: T): void {
    this.pending = value;
    if (this.timer) clearTimeout(this.timer);
    this.onState?.('pending');
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.enqueuePending();
    }, this.delayMs);
  }

  flush(): Promise<void> {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    return this.enqueuePending();
  }

  whenIdle(): Promise<void> {
    return this.chain;
  }

  private enqueuePending(): Promise<void> {
    if (this.pending === undefined) return this.chain;
    const snapshot = this.pending;
    this.pending = undefined;
    const run = this.chain
      .catch(() => undefined)
      .then(() => {
        this.onState?.('saving');
        return this.save(snapshot);
      })
      .then(() => {
        this.onState?.('saved');
      })
      .catch((error: unknown) => {
        this.onState?.('error');
        this.onError?.(error);
        throw error;
      });
    this.chain = run;
    return run;
  }
}
