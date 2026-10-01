<script setup lang="ts">
/**
 * 背景粒子动画（性能优先版 · 支持圆形 / 棱形 / 正方形三种形状）
 * 优化点：
 *  - 预渲染发光贴图（按形状 + 主题色缓存），避免每帧 shadowBlur（最大性能杀手）
 *  - 网格分桶计算连线，复杂度从 O(n²) 降到近似 O(n)
 *  - 粒子本体按亮度档位分桶，每档一次 path 批量填充（不逐粒子 fill）
 *  - 限帧 30fps；窗口失焦降到 10fps；窗口最小化 / 不可见 / 组件禁用时自动停帧
 *    （WebView2 在最小化时不会触发 document.hidden，必须走 Tauri 窗口事件判断）
 *  - 背景层采样倍率固定 1.0（柔光粒子不需要高 DPI），每帧填充像素少约 55%
 *  - 三种形状共用同一套粒子系统：只切换「绘制路径」，粒子位置与速度完全一致
 */
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { isTauri } from '@/services/ipc'

/** 粒子形状：圆形（实心圆点）/ 棱形（旋转 45° 的正方形）/ 正方形（方点） */
type ParticleShape = 'circle' | 'diamond' | 'square'

const props = withDefaults(
  defineProps<{
    enabled?: boolean
    quality?: string
    /** 粒子形状，取值 circle | diamond | square，非法值回退 circle */
    shape?: string
  }>(),
  { enabled: true, quality: 'medium', shape: 'circle' },
)

const SHAPES: ParticleShape[] = ['circle', 'diamond', 'square']

/** 容错归一化：后端字段缺失 / 尚未落库时一律按 circle 画，绝不空白 */
function shapeOf(v?: string): ParticleShape {
  const s = (v || '').trim().toLowerCase() as ParticleShape
  return SHAPES.includes(s) ? s : 'circle'
}

interface P {
  x: number
  y: number
  vx: number
  vy: number
  r: number
  /** 亮度档位下标（同一档的粒子合并成一条 path 一次填充） */
  k: number
  a: number
}

/** 亮度档位：把随机透明度离散成几档，换取批量填充 */
const ALPHAS = [0.32, 0.42, 0.52, 0.62]

const canvas = ref<HTMLCanvasElement | null>(null)

let ctx: CanvasRenderingContext2D | null = null
let particles: P[] = []
let raf = 0
let last = 0
let running = false
let sprite: HTMLCanvasElement | null = null
let spriteKey = ''
const SPRITE = 64
const GLOW = 6 // 光晕贴图绘制尺寸 = 半径 × 6
const reduced = typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches

const FRAME_MS = 1000 / 30 // 限帧 30fps，肉眼几乎无差别，CPU 占用大幅下降
const FRAME_MS_IDLE = 1000 / 10 // 窗口失焦时降到 10fps：背景动画还在动，开销约降到 1/3
let frameMs = FRAME_MS

/* ------------------------- 主题色（跟随 --umi-* 变量） ------------------------- */
const FALLBACK_RGB = '149,117,255' // 紫罗兰 a400，与 :root 默认强调色一致
let themeRgb = FALLBACK_RGB
let linkColor = `rgba(${FALLBACK_RGB},0.12)`
let dotColor = `rgb(${FALLBACK_RGB})`

/** "149 117 255" → "149,117,255"；不合法返回 null */
function parseTriple(raw: string): string | null {
  const s = (raw || '').trim().replace(/\s+/g, ' ')
  const m = /^(\d{1,3})[ ,]+(\d{1,3})[ ,]+(\d{1,3})$/.exec(s)
  return m ? `${+m[1]},${+m[2]},${+m[3]}` : null
}

/** "#7c4dff" → "124,77,255"；不合法返回 null */
function hexTriple(raw: string): string | null {
  const m = /^#?([\da-f]{2})([\da-f]{2})([\da-f]{2})$/i.exec((raw || '').trim())
  return m ? `${parseInt(m[1], 16)},${parseInt(m[2], 16)},${parseInt(m[3], 16)}` : null
}

/**
 * 读取当前主题的强调色。浅色 / 深色主题与切换强调色都会改写 <html> 上的
 * data-theme / data-accent，因此这里只需在属性变化时重新读一次即可。
 */
function readThemeColor() {
  if (typeof window === 'undefined' || typeof document === 'undefined' || !document.documentElement) return
  const cs = getComputedStyle(document.documentElement)
  const rgb =
    parseTriple(cs.getPropertyValue('--umi-a400')) ||
    parseTriple(cs.getPropertyValue('--umi-a500')) ||
    hexTriple(cs.getPropertyValue('--umi-accent-hex'))
  if (!rgb || rgb === themeRgb) return
  themeRgb = rgb
  linkColor = `rgba(${rgb},0.12)`
  dotColor = `rgb(${rgb})`
  spriteKey = '' // 颜色变了 → 贴图下一帧重建
  sprite = null
}

function density() {
  const q = props.quality || 'medium'
  if (q === 'low') return 110000
  if (q === 'high') return 16000
  return 34000
}

function maxCount() {
  const q = props.quality || 'medium'
  if (q === 'low') return 22
  if (q === 'high') return 78
  return 46
}

/** 按形状描一条粒子本体路径（三种形状唯一不同的地方） */
function traceDot(c: CanvasRenderingContext2D, shape: ParticleShape, p: P) {
  const r = p.r
  if (shape === 'square') {
    c.rect(p.x - r, p.y - r, r * 2, r * 2) // 正方形：边长 2r 的方点
    return
  }
  if (shape === 'diamond') {
    const d = r * 1.15 // 棱形：半对角线，面积与圆接近
    c.moveTo(p.x, p.y - d)
    c.lineTo(p.x + d, p.y)
    c.lineTo(p.x, p.y + d)
    c.lineTo(p.x - d, p.y)
    c.closePath()
    return
  }
  c.moveTo(p.x + r, p.y)
  c.arc(p.x, p.y, r, 0, Math.PI * 2) // 圆形：实心圆点
}

/**
 * 预渲染一张「形状 + 主题色」的发光贴图，用 drawImage 代替每帧 arc + shadowBlur。
 * 只按形状（circle / diamond / square）与颜色缓存，切换形状时重建一次即可。
 */
function makeSprite(shape: ParticleShape): HTMLCanvasElement | null {
  const c = document.createElement('canvas')
  c.width = SPRITE
  c.height = SPRITE
  const g = c.getContext('2d')
  if (!g) return null
  const r = SPRITE / 2
  const grad = g.createRadialGradient(r, r, 0, r, r, r)
  grad.addColorStop(0, `rgba(${themeRgb},0.9)`)
  grad.addColorStop(0.34, `rgba(${themeRgb},0.4)`)
  grad.addColorStop(1, `rgba(${themeRgb},0)`)
  g.fillStyle = grad
  const kr = r * 0.92
  if (shape === 'square') {
    g.fillRect(r - kr, r - kr, kr * 2, kr * 2)
  } else if (shape === 'diamond') {
    g.beginPath()
    g.moveTo(r, r - kr)
    g.lineTo(r + kr, r)
    g.lineTo(r, r + kr)
    g.lineTo(r - kr, r)
    g.closePath()
    g.fill()
  } else {
    g.beginPath()
    g.arc(r, r, kr, 0, Math.PI * 2)
    g.fill()
  }
  return c
}

function size() {
  const c = canvas.value
  if (!c) return
  // 背景层是柔光粒子（不是文字/线条），采样倍率固定 1.0：
  // 相比原来的 1.5 倍，每帧要填充的像素少约 55%，肉眼几乎分辨不出差别。
  const dpr = 1
  c.width = Math.max(1, Math.floor(c.clientWidth * dpr))
  c.height = Math.max(1, Math.floor(c.clientHeight * dpr))
  ctx = c.getContext('2d', { alpha: true })
  ctx?.setTransform(dpr, 0, 0, dpr, 0, 0)
}

function seed() {
  const c = canvas.value
  if (!c) return
  const n = Math.min(maxCount(), Math.max(8, Math.floor((c.clientWidth * c.clientHeight) / density())))
  particles = Array.from({ length: n }, () => {
    const k = Math.floor(Math.random() * ALPHAS.length)
    return {
      x: Math.random() * c.clientWidth,
      y: Math.random() * c.clientHeight,
      vx: (Math.random() - 0.5) * 0.3,
      vy: (Math.random() - 0.5) * 0.3,
      r: Math.random() * 1.8 + 0.9,
      k,
      a: ALPHAS[k],
    }
  })
}

const LINK = 118

function frame(now: number) {
  if (!running) return
  raf = requestAnimationFrame(frame)
  if (now - last < frameMs) return
  last = now

  const c = canvas.value
  if (!c || !ctx) return
  const W = c.clientWidth
  const H = c.clientHeight
  const shape = shapeOf(props.shape)
  ctx.clearRect(0, 0, W, H)
  const key = `${shape}|${themeRgb}`
  if (!sprite || spriteKey !== key) {
    sprite = makeSprite(shape)
    spriteKey = key
  }

  // 网格分桶，避免 O(n²)
  const cell = LINK
  const cols = Math.max(1, Math.ceil(W / cell))
  const grid = new Map<number, number[]>()
  for (let i = 0; i < particles.length; i++) {
    const p = particles[i]
    p.x += p.vx
    p.y += p.vy
    if (p.x < -12) p.x = W + 12
    if (p.x > W + 12) p.x = -12
    if (p.y < -12) p.y = H + 12
    if (p.y > H + 12) p.y = -12
    const key2 = Math.floor(p.x / cell) + Math.floor(p.y / cell) * cols
    const bucket = grid.get(key2)
    if (bucket) bucket.push(i)
    else grid.set(key2, [i])
  }

  ctx.lineWidth = 0.7
  ctx.strokeStyle = linkColor
  ctx.beginPath()
  for (const [key2, bucket] of grid) {
    const cx = key2 % cols
    const cy = Math.floor(key2 / cols)
    for (let dx = 0; dx <= 1; dx++) {
      for (let dy = dx === 0 ? 0 : -1; dy <= 1; dy++) {
        if (dx === 0 && dy < 0) continue
        const nk = cx + dx + (cy + dy) * cols
        if (nk === key2) continue
        const other = grid.get(nk)
        if (!other) continue
        for (const i of bucket) {
          for (const j of other) {
            const a = particles[i]
            const b = particles[j]
            const ddx = a.x - b.x
            const ddy = a.y - b.y
            if (Math.abs(ddx) > LINK || Math.abs(ddy) > LINK) continue
            const d = Math.sqrt(ddx * ddx + ddy * ddy)
            if (d < LINK) {
              ctx.moveTo(a.x, a.y)
              ctx.lineTo(b.x, b.y)
            }
          }
        }
      }
    }
  }
  ctx.stroke()

  // 光晕：预渲染贴图 + **按亮度档位合批**（4 次 globalAlpha 切换，而不是每颗粒子一次）
  if (sprite) {
    for (let k = 0; k < ALPHAS.length; k++) {
      let any = false
      for (const p of particles) {
        if (p.k !== k) continue
        if (!any) {
          any = true
          ctx.globalAlpha = ALPHAS[k] * 0.55
        }
        const s = p.r * GLOW
        ctx.drawImage(sprite, p.x - s / 2, p.y - s / 2, s, s)
      }
    }
    ctx.globalAlpha = 1
  }

  // 实心本体：按亮度档位分桶，每档一条 path 一次填充（形状差异只在这里）
  ctx.fillStyle = dotColor
  for (let k = 0; k < ALPHAS.length; k++) {
    let any = false
    ctx.beginPath()
    for (const p of particles) {
      if (p.k !== k) continue
      any = true
      traceDot(ctx, shape, p)
    }
    if (!any) continue
    ctx.globalAlpha = ALPHAS[k]
    ctx.fill()
  }
  ctx.globalAlpha = 1
}

function start() {
  if (running || !props.enabled || reduced) return
  running = true
  last = 0
  raf = requestAnimationFrame(frame)
}
function stop() {
  running = false
  cancelAnimationFrame(raf)
  const c = canvas.value
  if (c && ctx) ctx.clearRect(0, 0, c.clientWidth, c.clientHeight)
}

function onResize() {
  size()
  seed()
}

function onVisibility() {
  if (document.hidden) stop()
  else start()
}

/** 窗口失焦 → 降到 10fps（动画仍在跑，只是更省）；重新聚焦 → 回到 30fps */
function onFocus() {
  frameMs = FRAME_MS
}
function onBlur() {
  frameMs = FRAME_MS_IDLE
}

/** 主题 / 强调色切换时重新取色（切形状不需要重新播种，粒子位置保持连续） */
let themeMo: MutationObserver | null = null

function watchTheme() {
  if (typeof MutationObserver === 'undefined' || typeof document === 'undefined') return null
  const mo = new MutationObserver(() => readThemeColor())
  mo.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['data-theme', 'data-accent', 'class', 'style'],
  })
  return mo
}

let unlistenWin: Array<() => void> = []

/** 最小化时真正的停帧：WebView2 最小化不会置 document.hidden，只能问 Tauri 窗口状态 */
async function watchWindowState() {
  if (!isTauri()) return
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    const win = getCurrentWindow()
    const sync = async () => {
      const min = await win.isMinimized()
      if (min || document.hidden) stop()
      else start()
    }
    unlistenWin.push(
      await win.onFocusChanged(({ payload: focused }) => {
        frameMs = focused ? FRAME_MS : FRAME_MS_IDLE
        void sync()
      }),
    )
    unlistenWin.push(await win.onResized(() => void sync()))
  } catch {
    /* 非 Tauri 环境或窗口 API 不可用时忽略：仍然有 visibilitychange 兜底 */
  }
}

onMounted(() => {
  readThemeColor()
  size()
  seed()
  window.addEventListener('resize', onResize)
  document.addEventListener('visibilitychange', onVisibility)
  window.addEventListener('focus', onFocus)
  window.addEventListener('blur', onBlur)
  themeMo = watchTheme()
  void watchWindowState()
  start()
})

onBeforeUnmount(() => {
  stop()
  for (const un of unlistenWin) un()
  unlistenWin = []
  themeMo?.disconnect()
  themeMo = null
  window.removeEventListener('resize', onResize)
  document.removeEventListener('visibilitychange', onVisibility)
  window.removeEventListener('focus', onFocus)
  window.removeEventListener('blur', onBlur)
})

watch(
  () => [props.enabled, props.quality],
  () => {
    if (!props.enabled) {
      stop()
      return
    }
    seed()
    start()
  },
)

// 形状切换：只丢弃贴图，换路径重绘，粒子系统本身不动
watch(
  () => props.shape,
  () => {
    spriteKey = ''
    sprite = null
  },
)
</script>

<template>
  <canvas ref="canvas" class="pointer-events-none absolute inset-0 h-full w-full opacity-60" />
</template>
