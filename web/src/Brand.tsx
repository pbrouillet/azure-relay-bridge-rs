import { useContext } from 'react'
import { AppearanceContext } from './theme'

export function BrandArtwork({ asset, width, height, className, priority = false }: {
  asset: 'hero' | 'connections' | 'profiles' | 'gateways' | 'wordmark'
  width: number
  height: number
  className?: string
  priority?: boolean
}) {
  const theme = useContext(AppearanceContext)
  return <img src={`/brand/${asset}-${theme}.${asset === 'wordmark' ? 'svg' : 'webp'}`}
    alt="" width={width} height={height} className={className}
    loading={priority ? 'eager' : 'lazy'} decoding="async" />
}

function BrandMark({ decorative = false, size = 36 }: { decorative?: boolean; size?: number }) {
  return <svg className="brand-mark" viewBox="0 0 64 64" width={size} height={size} role={decorative ? undefined : 'img'} aria-label={decorative ? undefined : 'azbridge'} aria-hidden={decorative || undefined}>
      <path fill="var(--colorNeutralForeground1)" d="M8,40 8,8 40,8 40,16 16,16 16,40Z" />
      <path fill="var(--colorBrandForeground1)" d="M24,56 56,56 56,24 48,24 48,48 24,48Z" />
    </svg>
}

export function WelcomeLogo() {
  return <div className="signin-artwork" aria-hidden="true">
    <BrandMark decorative size={256} />
    <BrandArtwork asset="wordmark" width={300} height={88} priority />
  </div>
}

export function BrandLogo() {
  return <span className="brand-identity">
    <BrandMark />
    <BrandArtwork asset="wordmark" width={150} height={44} className="brand-wordmark" priority />
  </span>
}
