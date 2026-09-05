import anthropic from '../assets/brands/anthropic.svg';
import openaiBrand from '../assets/brands/openai.svg';
import bytedance from '../assets/brands/bytedance.svg';
import xiaomi from '../assets/brands/xiaomi.svg';
import alibabacloud from '../assets/brands/alibabacloud.svg';

/**
 * 服务商品牌标识（决策 2026-08-24 R-8 按原型对齐）：
 * 已知厂商用官方 SVG（与 prototype/assets/brands 同源），其余按原型用字标或通用图标。
 */

const BRAND_IMG: Record<string, string> = {
  anthropic,
  openai: openaiBrand,
  bytedance: bytedance,
  volc: bytedance,
  xiaomi,
  mimo: xiaomi,
  alibabacloud,
  bailian: alibabacloud,
};

/** 原型中的字标（无官方 SVG 的厂商） */
export function providerWordMark(providerId?: string): string | null {
  if (!providerId) return null;
  const map: Record<string, string> = {
    kimi: 'K',
    moonshot: 'K',
    glm: 'GLM',
    zhipu: 'GLM',
    minimax: 'MM',
    deepseek: 'DS',
    qwen: 'Q',
    alibaba: 'Q',
  };
  return map[providerId] ?? null;
}

export function ProviderBrand({ providerId, size = 22 }: { providerId?: string; size?: number }) {
  const img = providerId ? BRAND_IMG[providerId] : undefined;
  const word = providerWordMark(providerId);
  if (img) {
    return <img src={img} alt={providerId ?? ''} width={size} height={size} />;
  }
  if (word) {
    return (
      <span className="cm-brand--word" style={{ fontSize: Math.max(11, size / 2) }}>
        {word}
      </span>
    );
  }
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} aria-hidden="true" focusable="false">
      <path
        fill="currentColor"
        d="M12 3a4 4 0 0 1 4 4v1h1a3 3 0 0 1 3 3v6a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-6a3 3 0 0 1 3-3h1V7a4 4 0 0 1 4-4Zm0 2a2 2 0 0 0-2 2v1h4V7a2 2 0 0 0-2-2Z"
      />
    </svg>
  );
}
