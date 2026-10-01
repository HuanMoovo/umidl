/**
 * 品牌 LOGO
 *
 * 默认使用内置图（assets/logo-cat-256.webp 猫头标记 / assets/icon-cat-512.webp 完整大图）；
 * 用户在「设置 → 外观 → 自定义 LOGO」里可以换成自己的图片：后端把图片复制到
 * %APPDATA%/umi-downloader/branding/ 并写入 settings.json，这里只保存路径，
 * 并通过 asset 协议转成 WebView 能加载的 URL。
 *
 * 性能注记：内置图改用 WebP 并降采样（256 / 512），打包体积从 737 KB 降到约 51 KB；
 * 它们只用于侧栏 36px 标记与设置页 68px 预览框，像素尺寸已远超实际显示需求。
 */
import { computed, ref } from 'vue'
import { convertFileSrc } from '@tauri-apps/api/core'
import defaultMark from '@/assets/logo-cat-256.webp'
import defaultLogo from '@/assets/icon-cat-512.webp'
import { isTauri } from '@/services/ipc'

const customPath = ref('')
/** 变更计数：换图后用它在 URL 上打戳，避免 WebView 缓存旧图 */
const stamp = ref(0)

export const defaultMarkUrl: string = defaultMark
export const defaultLogoUrl: string = defaultLogo

/** 由设置同步过来的自定义路径（空串 = 用内置） */
export function setLogoPath(path?: string | null): void {
  const next = (path ?? '').trim()
  if (next !== customPath.value) {
    customPath.value = next
    stamp.value++
  }
}

export function currentLogoPath(): string {
  return customPath.value
}

/** 自定义 LOGO 的 URL；未设置或非 Tauri 环境返回空串 */
export function customLogoUrl(): string {
  if (!customPath.value || !isTauri()) return ''
  return `${convertFileSrc(customPath.value)}?v=${stamp.value}`
}

/** 各界面使用：有自定义用自定义，否则回退内置矢量图 */
export function useBranding() {
  const markUrl = computed(() => customLogoUrl() || defaultMarkUrl)
  const logoUrl = computed(() => customLogoUrl() || defaultLogoUrl)
  const isCustom = computed(() => !!customLogoUrl())
  return { markUrl, logoUrl, isCustom }
}
