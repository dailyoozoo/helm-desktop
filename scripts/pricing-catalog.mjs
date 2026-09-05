import { readFile } from 'node:fs/promises';
import process from 'node:process';

const catalogPath = new URL('../src-tauri/assets/pricing-catalog.json', import.meta.url);
const catalog = JSON.parse(await readFile(catalogPath, 'utf8'));

function fail(message) {
  throw new Error(`[pricing-catalog] ${message}`);
}

function validateBand(modelId, band) {
  for (const field of ['input', 'output']) {
    if (!Number.isFinite(band[field]) || band[field] < 0) fail(`${modelId}.${field} 非法`);
  }
  for (const field of ['cachedInput', 'cacheWrite']) {
    if (band[field] !== undefined && (!Number.isFinite(band[field]) || band[field] < 0)) {
      fail(`${modelId}.${field} 非法`);
    }
  }
  if (
    band.minInputTokens !== undefined &&
    band.maxInputTokens !== undefined &&
    band.minInputTokens > band.maxInputTokens
  ) {
    fail(`${modelId} 的上下文价格区间颠倒`);
  }
}

function validateBandRanges(modelId, bands) {
  let previousMax;
  for (let index = 0; index < bands.length; index += 1) {
    const band = bands[index];
    if (index === 0) {
      if ((band.minInputTokens ?? 0) !== 0) fail(`${modelId} 首个价格区间没有从 0 开始`);
    } else {
      if (previousMax === undefined || band.minInputTokens !== previousMax + 1) {
        fail(`${modelId} 价格区间存在空洞或重叠`);
      }
    }
    if (index + 1 < bands.length && band.maxInputTokens === undefined) {
      fail(`${modelId} 非末尾价格区间缺少上限`);
    }
    previousMax = band.maxInputTokens;
  }
}

function validateCatalog(value) {
  if (value.schemaVersion !== 1) fail(`不支持 schemaVersion=${value.schemaVersion}`);
  if (!value.catalogVersion || !value.publishedAt || !Number.isInteger(value.sequence)) {
    fail('缺少 catalogVersion、publishedAt 或整数 sequence');
  }
  if (!Array.isArray(value.models) || value.models.length === 0 || value.models.length > 10_000) {
    fail('模型数量无效');
  }
  const ids = new Set();
  for (const model of value.models) {
    const vendor = String(model.vendor).toLowerCase();
    const modelIds = [model.modelId, ...(model.aliases ?? [])];
    for (const modelId of modelIds) {
      const normalized = String(modelId)
        .trim()
        .toLowerCase()
        .replaceAll('@', '-')
        .replace(/^(models\/|anthropic\/|openai\/)/, '');
      if (!normalized) fail(`${model.modelId} 包含空模型 ID 或别名`);
      const identity = `${vendor}:${normalized}`;
      if (ids.has(identity)) fail(`重复模型或别名 ${identity}`);
      ids.add(identity);
    }
    const identity = `${vendor}:${String(model.modelId).toLowerCase()}`;
    if (model.currency !== 'USD' || model.unit !== 'per-million-tokens') {
      fail(`${identity} 的币种或单位无效`);
    }
    if (!model.sourceUrl || !model.observedAt || !model.tiers?.standard?.bands?.length) {
      fail(`${identity} 缺少来源、观察时间或 standard 价格`);
    }
    for (const tier of Object.values(model.tiers)) {
      if (!Array.isArray(tier.bands) || tier.bands.length === 0) fail(`${identity} 存在空 tier`);
      validateBandRanges(identity, tier.bands);
      for (const band of tier.bands) validateBand(identity, band);
    }
  }
  return ids;
}

function standardBase(model) {
  return model?.tiers?.standard?.bands?.[0];
}

/* 上游对账（models.dev 社区库，官方牌价口径）：字段同构（input/output/cache_read/cache_write，
  均为每百万 token），只对厂商官方入口比对；聚合中转报价不入库。 */
const UPSTREAM_VENDORS = {
  anthropic: { provider: 'anthropic', idFilter: null },
  openai: { provider: 'openai', idFilter: /^gpt-/ },
  deepseek: { provider: 'deepseek', idFilter: null },
  moonshot: { provider: 'moonshot', idFilter: null },
};
async function checkUpstream() {
  const response = await fetch('https://models.dev/api.json', {
    headers: { accept: 'application/json', 'user-agent': 'helm-pricing-audit/1' },
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) fail(`models.dev 返回 HTTP ${response.status}`);
  const providers = await response.json();
  let missingTotal = 0;
  const drift = [];
  for (const [vendor, rule] of Object.entries(UPSTREAM_VENDORS)) {
    const upstreamModels = providers[rule.provider]?.models ?? {};
    if (!Object.keys(upstreamModels).length) {
      console.log(`[pricing-catalog] 上游缺少 ${rule.provider} 入口，跳过`);
      continue;
    }
    const localByVendor = new Map(
      catalog.models
        .filter((model) => model.vendor === vendor)
        .map((model) => [model.modelId, model]),
    );
    const missing = [];
    for (const [modelId, upstream] of Object.entries(upstreamModels)) {
      if (rule.idFilter && !rule.idFilter.test(modelId)) continue;
      const local = localByVendor.get(modelId);
      if (!local) {
        missing.push(modelId);
        continue;
      }
      const base = standardBase(local);
      const cost = upstream.cost ?? {};
      if (!Number.isFinite(cost.input) || !Number.isFinite(cost.output)) continue;
      const drifted =
        base.input !== cost.input ||
        base.output !== cost.output ||
        (Number.isFinite(cost.cache_read) && base.cachedInput !== cost.cache_read) ||
        (Number.isFinite(cost.cache_write) && base.cacheWrite !== cost.cache_write);
      if (drifted) {
        drift.push({
          vendor,
          modelId,
          local: [base.input, base.cachedInput ?? null, base.output],
          upstream: [cost.input, cost.cache_read ?? null, cost.output],
        });
      }
    }
    if (missing.length) {
      missingTotal += missing.length;
      console.log(`[pricing-catalog] 上游发现 ${missing.length} 个未收录 ${vendor} 模型：`);
      console.log(missing.sort().join('\n'));
    }
  }
  if (drift.length) {
    console.log('[pricing-catalog] 上游价格差异（仅提示，必须回到官方来源审核）：');
    console.log(JSON.stringify(drift, null, 2));
  }
  if (missingTotal || drift.length) process.exitCode = 2;
}

validateCatalog(catalog);
console.log(
  `[pricing-catalog] ${catalog.catalogVersion} 校验通过，共 ${catalog.models.length} 个模型`,
);

if (process.argv.includes('--check-upstream')) await checkUpstream();
