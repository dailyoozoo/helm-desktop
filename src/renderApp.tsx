import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { ErrorBoundary } from './shell/ErrorBoundary';
import './styles/app.css';
import './workspace/workspace.css';

export function mountApp() {
  const root = document.getElementById('root');
  if (!root) return;
  createRoot(root).render(
    <StrictMode>
      <ErrorBoundary label="应用">
        <App />
      </ErrorBoundary>
    </StrictMode>,
  );
}
