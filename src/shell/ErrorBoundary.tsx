// 全局/页面级错误边界：任一组件渲染抛异常时显示可恢复的错误页，
// 而不是整个应用白屏（Tauri 没有刷新按钮，白屏等于死机）。
import { Component, type ErrorInfo, type ReactNode } from 'react';

interface ErrorBoundaryProps {
  /** 边界名称，用于诊断信息（如页面名） */
  label: string;
  /** 点击「返回总览」时的回调；App 级边界可不传（无处可去时只提供重试） */
  onNavigateHome?: () => void;
  children: ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
  /** 「复制诊断信息」结果反馈（边界可能包裹全应用，不依赖全局 toast） */
  copyState: 'idle' | 'ok' | 'fail';
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  override state: ErrorBoundaryState = { error: null, copyState: 'idle' };

  static getDerivedStateFromError(error: Error): Partial<ErrorBoundaryState> {
    return { error, copyState: 'idle' };
  }

  override componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[ErrorBoundary:${this.props.label}]`, error, info.componentStack);
  }

  private handleRetry = () => {
    this.setState({ error: null, copyState: 'idle' });
  };

  private handleNavigateHome = () => {
    this.setState({ error: null, copyState: 'idle' });
    this.props.onNavigateHome?.();
  };

  private handleCopyDiagnostics = () => {
    const { error } = this.state;
    const text = [
      `位置：${this.props.label}`,
      `错误：${error?.message ?? '未知错误'}`,
      error?.stack ?? '',
    ].join('\n');
    void navigator.clipboard
      ?.writeText(text)
      .then(() => this.setState({ copyState: 'ok' }))
      .catch(() => this.setState({ copyState: 'fail' }));
  };

  override render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <main className="main">
        <div className="page scroll">
          <div className="error-boundary">
            <div className="error-boundary__title">界面出现异常</div>
            <div className="error-boundary__message">
              「{this.props.label}」渲染时发生错误：{error.message || '未知错误'}
            </div>
            <div className="error-boundary__actions">
              <button type="button" className="btn" onClick={this.handleRetry}>
                重试
              </button>
              {this.props.onNavigateHome ? (
                <button type="button" className="btn" onClick={this.handleNavigateHome}>
                  返回总览
                </button>
              ) : null}
              <button type="button" className="btn" onClick={this.handleCopyDiagnostics}>
                {this.state.copyState === 'ok'
                  ? '已复制'
                  : this.state.copyState === 'fail'
                    ? '复制失败，请截图'
                    : '复制诊断信息'}
              </button>
            </div>
          </div>
        </div>
      </main>
    );
  }
}
