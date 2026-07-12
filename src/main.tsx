import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './styles/app.css';
import './workspace/workspace.css';
import { App } from './App';
import { ErrorBoundary } from './shell/ErrorBoundary';

const root = document.getElementById('root');
if (root) {
  createRoot(root).render(
    <StrictMode>
      <ErrorBoundary label="应用">
        <App />
      </ErrorBoundary>
    </StrictMode>,
  );
}
