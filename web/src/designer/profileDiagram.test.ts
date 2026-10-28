import { describe, expect, it } from 'vitest'
import client from '../import/fixtures/client.yml?raw'
import server from '../import/fixtures/server.yml?raw'
import { convertStandalone } from '../import/standaloneConfig'
import { buildDiagram, diagramPageSize } from './profileDiagram'
import { parseProfile } from './profileModel'

describe('profile diagram', () => {
  it.each([['client', client, 16, 'LocalForward'], ['server', server, 10, 'RemoteForward']] as const)(
    'pages every %s binding without changing the imported configuration', (name, source, count, direction) => {
      const imported = convertStandalone(`${name}.yml`, source)
      const originalYaml = imported.yaml
      const bindings = parseProfile(imported.yaml).model.bindings
      expect(bindings).toEqual(imported.bindings)
      expect(bindings).toHaveLength(count)
      expect(bindings.every(binding => binding.direction === direction)).toBe(true)
      const rendered = []
      for (let start = 0; start < count; start += diagramPageSize) {
        const page = bindings.slice(start, start + diagramPageSize)
        const diagram = buildDiagram(page, imported.relayNames, false, '')
        expect(diagram.edges).toHaveLength(page.length)
        expect(diagram.nodes.filter(node => node.data.relay)).toHaveLength(1)
        expect(diagram.edges.every(edge => edge.label === undefined)).toBe(true)
        for (const binding of page) {
          const node = diagram.nodes.find(node => node.id === binding.id)!
          expect(node.data).toMatchObject({ port: binding.port, portName: binding.portName, label: `${binding.protocol} ${direction === 'LocalForward' ? 'listener' : 'destination'}` })
          expect(node.position.y + 132).toBeLessThan(diagram.height)
          expect(node.position.x + 280).toBeLessThan(diagram.width)
          rendered.push(node.id)
        }
      }
      expect(rendered).toEqual(bindings.map(binding => binding.id))
      expect(imported.yaml).toBe(originalYaml)
      expect(parseProfile(imported.yaml).model.bindings).toEqual(bindings)
    },
  )

  it('centers the used relay, separates additional relays and controls reconnect direction', () => {
    const bindings = convertStandalone('client.yml', client).bindings.slice(0, diagramPageSize)
    const diagram = buildDiagram(bindings, ['example-relay', 'second'], false, 'second')
    expect(diagram.nodes.find(node => node.id === 'relay:example-relay')?.position).toEqual({ x: 400, y: 240 })
    expect(diagram.nodes.find(node => node.id === 'relay:second')).toMatchObject({ position: { x: 400, y: 400 }, data: { detail: 'Additional connection', warning: false } })
    expect(diagram.edges.every(edge => edge.reconnectable === 'target')).toBe(true)
    expect(buildDiagram(bindings, [], true, '').edges.every(edge => edge.reconnectable === false)).toBe(true)
    expect(buildDiagram(bindings, [], true, '').nodes.find(node => node.data.relay)?.data.warning).toBe(true)
    expect(buildDiagram(bindings, ['example-relay'], false, 'example-relay').nodes.filter(node => node.data.relay)).toHaveLength(1)
    expect(buildDiagram(bindings, ['example-relay', 'second'], false, 'second')).toEqual(diagram)
  })

  it('lays out mixed directions and distinct relays without overlapping relay cards', () => {
    const source = 'LocalForward:\n- RelayName: a\n  BindPort: -53\n- RelayName: b\n  BindLocalSocket: /tmp/local\nRemoteForward:\n- RelayName: c\n  Host: backend.example.test\n  HostPort: 443\n  Http: true\n'
    const bindings = parseProfile(source).model.bindings
    const diagram = buildDiagram(bindings, ['a', 'b', 'c'], false, '')
    const relays = diagram.nodes.filter(node => node.data.relay)
    expect(relays.map(node => node.position.y)).toEqual([0, 160, 320])
    expect(diagram.nodes.find(node => node.id === bindings[0].id)).toMatchObject({ position: { x: 0, y: 0 }, data: { label: 'UDP listener', port: 53, detail: 'default address' } })
    expect(diagram.nodes.find(node => node.id === bindings[1].id)?.data).toMatchObject({ label: 'Unix socket listener', detail: '/tmp/local', port: undefined })
    expect(diagram.nodes.find(node => node.id === bindings[2].id)).toMatchObject({ position: { x: 800, y: 0 }, data: { label: 'HTTP destination', detail: 'backend.example.test', port: 443 } })
    expect(diagram.edges.at(-1)).toMatchObject({ source: 'relay:c', target: bindings[2].id, reconnectable: 'source' })
    expect(relays.every(node => node.position.y + 132 < diagram.height)).toBe(true)
  })

  it('handles empty profiles and a selectable relay without inventing rules', () => {
    expect(buildDiagram([], [], false, '')).toEqual({ nodes: [], edges: [], width: 360, height: 460 })
    const diagram = buildDiagram([], ['ready'], false, 'ready')
    expect(diagram.nodes).toHaveLength(1)
    expect(diagram.nodes[0]).toMatchObject({ id: 'relay:ready', position: { x: 0, y: 0 }, data: { warning: false } })
    expect(diagram.edges).toEqual([])
  })
})
