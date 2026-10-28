export const maxProfileBytes = 1024 * 1024

export async function readProfileFile(file: File): Promise<string> {
  if (!/\.ya?ml$/i.test(file.name)) throw new Error('Choose a .yaml or .yml file.')
  if (file.size > maxProfileBytes) throw new Error('YAML files must not exceed 1 MiB.')
  if (!file.size) throw new Error('The YAML file is empty. Your draft was not changed.')
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onerror = () => reject(new Error('Unable to read the YAML file. Your draft was not changed.'))
    reader.onabort = () => reject(new Error('YAML upload was cancelled. Your draft was not changed.'))
    reader.onload = () => {
      if (!(reader.result instanceof ArrayBuffer)) {
        reject(new Error('Unable to read the YAML file. Your draft was not changed.'))
        return
      }
      try {
        const yaml = new TextDecoder('utf-8', { fatal: true }).decode(reader.result)
        if (!yaml.trim()) throw new Error('The YAML file is empty. Your draft was not changed.')
        resolve(yaml)
      } catch (error) {
        reject(error instanceof TypeError ? new Error('The YAML file must use UTF-8 encoding. Your draft was not changed.') : error)
      }
    }
    reader.readAsArrayBuffer(file)
  })
}

export function downloadProfileYaml(yaml: string, profileName: string) {
  const safeName = profileName.replace(/[^a-zA-Z0-9_-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 100) || 'profile'
  const url = URL.createObjectURL(new Blob([yaml], { type: 'application/yaml;charset=utf-8' }))
  const link = document.createElement('a')
  link.href = url
  link.download = `${safeName}-draft.yaml`
  try {
    document.body.append(link)
    link.click()
  } finally {
    link.remove()
    window.setTimeout(() => URL.revokeObjectURL(url), 0)
  }
}
