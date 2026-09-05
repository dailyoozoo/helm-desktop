import { parseAnsi } from './parseAnsi';

/**
 * 把可能含 ANSI 转义的文本渲染为带样式的 React 节点。
 * 复用 parseAnsi 切成 token，**不用 innerHTML**，转义交给 React，天然免疫 XSS。
 * 无转义时退化为纯文本片段，零额外节点开销。
 */
export function AnsiText({ text, className }: { text: string; className?: string }) {
  const tokens = parseAnsi(text);
  // 无样式 token（纯文本）直接返回原串，避免无谓的 <span> 嵌套
  if (tokens.length === 1) {
    const only = tokens[0];
    if (!only.color && !only.bg && !only.bold && !only.italic && !only.underline) {
      return <>{only.text}</>;
    }
  }
  return (
    <span className={className}>
      {tokens.map((token, index) => (
        <span
          key={index}
          style={{
            ...(token.color ? { color: token.color } : {}),
            ...(token.bg ? { background: token.bg } : {}),
            ...(token.bold ? { fontWeight: 500 } : {}),
            ...(token.italic ? { fontStyle: 'italic' } : {}),
            ...(token.underline ? { textDecoration: 'underline' } : {}),
          }}
        >
          {token.text}
        </span>
      ))}
    </span>
  );
}
