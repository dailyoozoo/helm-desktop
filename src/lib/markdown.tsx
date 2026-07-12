import { memo, useState, type ReactNode } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';
import { Icon } from '../shell/icons';
import 'highlight.js/styles/github.css';
import './markdown.css';

// 助手/用户文本来自真实 CLI 进程，可能含任意字符。react-markdown 把 Markdown 编译成
// React 节点树（不经过 innerHTML），天然免疫 XSS；这里只做完整 GFM + 语法高亮 +
// 代码块复制按钮，不再自研极简解析（变更-10：自研达不到商业水准且难覆盖代码块/表格/列表）。

export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // 回退到 execCommand
  }
  try {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand('copy');
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}

function textOf(node: ReactNode): string {
  if (node == null || node === false || node === true) return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  if (typeof node === 'object' && 'props' in node) {
    return textOf((node as { props: { children?: ReactNode } }).props.children);
  }
  return '';
}

function CodeBlock({ children }: { children?: ReactNode }) {
  const [copied, setCopied] = useState(false);
  const code = textOf(children).replace(/\n$/, '');
  return (
    <div className="md-code">
      <button
        type="button"
        className="md-code__copy"
        title="复制代码"
        aria-label="复制代码"
        onClick={async () => {
          if (await copyText(code)) {
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1500);
          }
        }}
      >
        <Icon name={copied ? 'checkc' : 'copy'} />
        {copied ? '已复制' : '复制'}
      </button>
      <pre>{children}</pre>
    </div>
  );
}

export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[[rehypeHighlight, { detect: true, ignoreMissing: true }]]}
        components={{
          // 围栏代码块（pre>code）挂复制按钮；行内 code 保持默认
          pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
          // 外链在桌面端用系统浏览器打开，且不泄漏 referrer
          a: ({ href, children }) => (
            <a href={href} target="_blank" rel="noreferrer noopener">
              {children}
            </a>
          ),
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
