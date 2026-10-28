import { waitFor } from '@testing-library/react'
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest'
import { downloadProfileYaml, maxProfileBytes, readProfileFile } from './profileFiles'

beforeAll(() => {
  vi.stubGlobal('URL', class extends URL {
    static createObjectURL = vi.fn(() => 'blob:profile-download')
    static revokeObjectURL = vi.fn()
  })
})
afterAll(() => vi.unstubAllGlobals())

describe('profile YAML files', () => {
  it('reads UTF-8 YAML, accepting either extension and preserving line endings', async () => {
    const yaml = '# caf\u00e9\r\nLocalForward: []\r\n'
    expect(await readProfileFile(new File([yaml], 'PROFILE.YML'))).toBe(yaml)
    expect(await readProfileFile(new File(['\uFEFF', yaml], 'profile.yaml'))).toBe(yaml)
  })

  it('rejects non-YAML file names', async () => {
    await expect(readProfileFile(new File(['LocalForward: []'], 'profile.txt'))).rejects.toThrow('Choose a .yaml or .yml')
  })

  it('rejects files over the broker limit', async () => {
    await expect(readProfileFile(new File([new Uint8Array(maxProfileBytes + 1)], 'profile.yaml'))).rejects.toThrow('1 MiB')
  })

  it('accepts a file exactly at the size limit', async () => {
    const yaml = '#'.padEnd(maxProfileBytes, ' ')
    expect(await readProfileFile(new File([yaml], 'profile.yaml'))).toBe(yaml)
  })

  it('rejects empty and whitespace-only files', async () => {
    await expect(readProfileFile(new File([], 'profile.yaml'))).rejects.toThrow('empty')
    await expect(readProfileFile(new File([' \r\n\t'], 'profile.yaml'))).rejects.toThrow('empty')
  })

  it('rejects invalid UTF-8 instead of corrupting a draft', async () => {
    await expect(readProfileFile(new File([new Uint8Array([0xff, 0xfe, 0xff])], 'profile.yaml'))).rejects.toThrow('UTF-8')
  })

  it.each(['error', 'abort'])('reports file-reader %s events', async event => {
    vi.spyOn(FileReader.prototype, 'readAsArrayBuffer').mockImplementation(function (this: FileReader) {
      this.dispatchEvent(new ProgressEvent(event))
    })
    await expect(readProfileFile(new File(['LocalForward: []'], 'profile.yaml'))).rejects.toThrow(event === 'error' ? 'Unable to read' : 'cancelled')
  })

  it('reports an unexpected file-reader result', async () => {
    vi.spyOn(FileReader.prototype, 'readAsArrayBuffer').mockImplementation(function (this: FileReader) {
      this.dispatchEvent(new ProgressEvent('load'))
    })
    await expect(readProfileFile(new File(['LocalForward: []'], 'profile.yaml'))).rejects.toThrow('Unable to read')
  })

  it('downloads exact draft bytes with a safe YAML filename and revokes the URL', async () => {
    let filename = ''
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
      filename = this.download
      expect(this.href).toBe('blob:profile-download')
      expect(this.isConnected).toBe(true)
    })
    const yaml = '# draft\r\nLocalForward: []\r\n'
    downloadProfileYaml(yaml, '../../SSH profile')
    expect(filename).toBe('SSH-profile-draft.yaml')
    const blob = vi.mocked(URL.createObjectURL).mock.calls[0][0]
    expect(blob).toBeInstanceOf(Blob)
    if (!(blob instanceof Blob)) throw new Error('Expected a downloadable YAML Blob')
    expect(blob.size).toBe(new TextEncoder().encode(yaml).length)
    expect(blob.type).toBe('application/yaml;charset=utf-8')
    const content = await new Promise(resolve => {
      const reader = new FileReader()
      reader.onload = () => resolve(reader.result)
      reader.readAsText(blob)
    })
    expect(content).toBe(yaml)
    expect(document.querySelector('a[download]')).not.toBeInTheDocument()
    await waitFor(() => expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:profile-download'))
  })

  it('uses a fallback filename and cleans up when download initiation fails', async () => {
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
      expect(this.download).toBe('profile-draft.yaml')
      throw new Error('Download blocked')
    })
    expect(() => downloadProfileYaml('LocalForward: []', '...')).toThrow('Download blocked')
    expect(document.querySelector('a[download]')).not.toBeInTheDocument()
    await waitFor(() => expect(URL.revokeObjectURL).toHaveBeenCalled())
  })
})
