/**
 * 背景粒子形状回归测试
 *
 * 用「录制型 canvas 上下文」驱动真实组件：jsdom 没有 canvas 实现，
 * 这里给 getContext 打桩，把每帧的绘制命令记下来，从而验证
 *  - 圆形 / 棱形 / 正方形确实走不同的绘制路径（arc / 4 边菱形 / rect）
 *  - 粒子颜色取自主题 CSS 变量，切换强调色后跟着变
 *  - 三种形状共用同一套粒子系统（粒子数、光晕绘制次数一致）
 *  - 非法或缺失的 shape 值回退圆形
 */
import { describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, ref, type App } from 'vue'
import ParticleBg from '@/components/ParticleBg.vue'

/** 录制型 2D 上下文：把每个绘制调用记成 "name(args)" 字符串 */
function makeCtx() {
  const calls: string[] = []
  const rec = (name: string) =>
    (...args: unknown[]) =>
      void calls.push(`${name}(${args.map((a) => (typeof a === 'number' ? a.toFixed(2) : String(a))).join(',')})`)
  const ctx: Record<string, unknown> = {
    calls,
    lineWidth: 0,
    globalAlpha: 1,
    setTransform: rec('setTransform'),
    clearRect: rec('clearRect'),
    beginPath: rec('beginPath'),
    moveTo: rec('moveTo'),
    lineTo: rec('lineTo'),
    arc: rec('arc'),
    rect: rec('rect'),
    fillRect: rec('fillRect'),
    closePath: rec('closePath'),
    fill: rec('fill'),
    stroke: rec('stroke'),
    drawImage: rec('drawImage'),
    createRadialGradient: () => {
      calls.push('createRadialGradient')
      return { addColorStop: rec('addColorStop') }
    },
  }
  let fillStyle = ''
  let strokeStyle = ''
  Object.defineProperty(ctx, 'fillStyle', {
    get: () => fillStyle,
    set: (v: string) => {
      fillStyle = v
      calls.push(`fillStyle=${v}`)
    },
  })
  Object.defineProperty(ctx, 'strokeStyle', {
    get: () => strokeStyle,
    set: (v: string) => {
      strokeStyle = v
      calls.push(`strokeStyle=${v}`)
    },
  })
  return ctx as Record<string, unknown> & { calls: string[] }
}

let rafQueue: FrameRequestCallback[] = []

/** 主画布（在文档里）用录制上下文；预渲染贴图的离屏画布单独隔离，避免污染记录 */
function installCanvasStub(mainCtx: ReturnType<typeof makeCtx>) {
  const spriteCtx = makeCtx()
  ;(HTMLCanvasElement.prototype as unknown as { getContext: (id: string) => unknown }).getContext = function (
    this: HTMLCanvasElement,
  ) {
    return document.body.contains(this) ? mainCtx : spriteCtx
  }
  Object.defineProperty(HTMLCanvasElement.prototype, 'clientWidth', { configurable: true, get: () => 400 })
  Object.defineProperty(HTMLCanvasElement.prototype, 'clientHeight', { configurable: true, get: () => 300 })
}

function stubRaf() {
  rafQueue = []
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
    rafQueue.push(cb)
    return rafQueue.length
  })
  vi.stubGlobal('cancelAnimationFrame', () => {})
}

/** 模拟主题令牌：--umi-a400 = "<r> <g> <b>" */
function stubAccent(rgb = '34 211 238') {
  vi.stubGlobal('getComputedStyle', () => ({
    getPropertyValue: (n: string) => (n === '--umi-a400' ? rgb : ''),
  }))
}

/** 取「最后一帧连线之后」的路径操作（即粒子本体，不含连线） */
function dotOpsNow(calls: string[]): string[] {
  const tail = calls.slice(calls.lastIndexOf('stroke()') + 1)
  return tail.filter((c) => /^(arc|rect|lineTo|moveTo|closePath)\(/.test(c))
}

/** 取「最后一帧连线之后」的光晕绘制次数（= 粒子数） */
function glowNow(calls: string[]): number {
  return calls.slice(calls.lastIndexOf('stroke()') + 1).filter((c) => c.startsWith('drawImage(')).length
}

type Probe = {
  app: App
  setter: (v: string) => void
  dotOps: string[]
  glow: number
  fillStyle: string[]
  strokeStyle: string[]
}

function mountProbe(initialShape = 'circle'): Probe {
  const host = document.createElement('div')
  document.body.appendChild(host)
  const ctx = makeCtx()
  installCanvasStub(ctx)
  const shape = ref(initialShape)
  const app = createApp({
    setup: () => () => h(ParticleBg, { enabled: true, quality: 'medium', shape: shape.value }),
  })
  app.mount(host)
  const calls = ctx.calls
  return {
    app,
    setter: (v: string) => void (shape.value = v),
    get dotOps() {
      return dotOpsNow(calls)
    },
    get glow() {
      return glowNow(calls)
    },
    get fillStyle() {
      return calls.filter((c) => c.startsWith('fillStyle='))
    },
    get strokeStyle() {
      return calls.filter((c) => c.startsWith('strokeStyle='))
    },
  }
}

/** 单调递增的帧时间戳（组件限帧 30fps，要求 now 递增） */
let clock = 0
function tick(frames = 4) {
  for (let i = 0; i < frames; i++) {
    clock += 40
    const cb = rafQueue.shift()
    if (cb) cb(clock)
  }
}

function probe(shape: 'circle' | 'diamond' | 'square'): Probe {
  stubRaf()
  stubAccent()
  return mountProbe(shape)
}

describe('ParticleBg 粒子形状', () => {
  it('圆形=arc、棱形=菱形（3 边 + closePath 收口）、正方形=rect，三者绘制路径互不相同', () => {
    const circle = probe('circle')
    tick(4)
    const diamond = probe('diamond')
    tick(4)
    const square = probe('square')
    tick(4)

    expect(circle.dotOps.some((c) => c.startsWith('arc('))).toBe(true)
    expect(circle.dotOps.some((c) => c.startsWith('rect(') || c.startsWith('lineTo('))).toBe(false)

    const dLineTo = diamond.dotOps.filter((c) => c.startsWith('lineTo(')).length
    const dClose = diamond.dotOps.filter((c) => c === 'closePath()').length
    expect(dClose).toBeGreaterThanOrEqual(4)
    expect(dLineTo).toBe(dClose * 3) // 三条显式边 + closePath 收口 = 旋转 45° 的正方形
    expect(diamond.dotOps.some((c) => c.startsWith('arc(') || c.startsWith('rect('))).toBe(false)

    expect(square.dotOps.some((c) => c.startsWith('rect('))).toBe(true)
    expect(square.dotOps.some((c) => c.startsWith('arc('))).toBe(false)

    const sig = [circle, diamond, square].map((p) => [...new Set(p.dotOps.map((c) => c.split('(')[0]))].sort().join('+'))
    expect(sig).toEqual(['arc+moveTo', 'closePath+lineTo+moveTo', 'rect'])
    ;[circle, diamond, square].forEach((p) => p.app.unmount())
  })

  it('颜色取自主题 CSS 变量（--umi-a400 = 34 211 238）', () => {
    const circle = probe('circle')
    tick(4)
    expect(circle.fillStyle).toContain('fillStyle=rgb(34,211,238)')
    expect(circle.strokeStyle).toContain('strokeStyle=rgba(34,211,238,0.12)')
    circle.app.unmount()
  })

  it('切换强调色（data-accent 变化）后颜色跟着变', async () => {
    const p = probe('circle')
    tick(4)
    expect(p.fillStyle).toContain('fillStyle=rgb(34,211,238)')
    stubAccent('244 114 182') // 樱花粉
    document.documentElement.dataset.accent = 'pink'
    await new Promise((r) => setTimeout(r, 0)) // 等 MutationObserver 回调
    tick(4)
    expect(p.fillStyle.slice(-3)).toContain('fillStyle=rgb(244,114,182)')
    p.app.unmount()
  })

  it('三种形状共用同一套粒子系统：每帧光晕绘制次数（=粒子数）完全一致', () => {
    const counts = (['circle', 'diamond', 'square'] as const).map((s) => {
      const p = probe(s)
      tick(4)
      const n = p.glow
      p.app.unmount()
      return n
    })
    expect(counts[0]).toBeGreaterThan(0)
    expect(new Set(counts).size).toBe(1)
  })

  it('运行时切换形状：下一帧立刻改用新路径绘制', async () => {
    const p = probe('circle')
    tick(4)
    expect(p.dotOps.some((c) => c.startsWith('arc('))).toBe(true)
    p.setter('square')
    await nextTick()
    tick(4)
    expect(p.dotOps.some((c) => c.startsWith('rect('))).toBe(true)
    expect(p.dotOps.some((c) => c.startsWith('arc('))).toBe(false)
    p.app.unmount()
  })

  it('非法 / 缺失的形状值回退圆形', async () => {
    stubRaf()
    stubAccent()
    const host = document.createElement('div')
    document.body.appendChild(host)
    const ctx = makeCtx()
    installCanvasStub(ctx)
    const shape = ref('bogus')
    const app = createApp({
      setup: () => () => h(ParticleBg, { enabled: true, quality: 'medium', shape: shape.value }),
    })
    app.mount(host)
    tick(4)
    shape.value = undefined as unknown as string
    await nextTick()
    tick(4)
    expect(dotOpsNow(ctx.calls).some((c) => c.startsWith('arc('))).toBe(true)
    app.unmount()
  })
})
