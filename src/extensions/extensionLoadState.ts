export function applyExtensionLoadResult<T>(
  result: PromiseSettledResult<T>,
  setter: (value: T) => void,
  label: string,
  failures: string[],
) {
  if (result.status === 'fulfilled') {
    setter(result.value);
    return;
  }
  console.error(`加载${label}失败:`, result.reason);
  failures.push(label);
}
