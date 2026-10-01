import { createRouter, createWebHashHistory } from 'vue-router'

const routes = [
  { path: '/', name: 'home', component: () => import('@/views/Home.vue'), meta: { title: '首页' } },
  { path: '/download', name: 'download', component: () => import('@/views/Download.vue'), meta: { title: '下载' } },
  { path: '/converter', name: 'converter', component: () => import('@/views/Converter.vue'), meta: { title: '转换' } },
  { path: '/subtitle', name: 'subtitle', component: () => import('@/views/Subtitle.vue'), meta: { title: '字幕' } },
  // 插件（1.6）已并入设置页：旧链接 #/plugins 保留但重定向到「设置 → 插件」页签，避免死链
  {
    path: '/plugins',
    name: 'plugins',
    redirect: { path: '/settings', query: { tab: 'plugins' } },
    meta: { title: '插件' },
  },
  { path: '/settings', name: 'settings', component: () => import('@/views/Settings.vue'), meta: { title: '设置' } },
]

export default createRouter({
  history: createWebHashHistory(),
  routes,
})
