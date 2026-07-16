'use client'

import Image from 'next/image'
import { useState } from 'react'

import type { SupportedService } from '@/lib/types/gateway'
import {
  SERVICE_BRANDS,
  SERVICE_BRAND_FALLBACK,
  SERVICE_LOGOS,
  SERVICE_SVG_FALLBACKS,
  isServiceKey,
} from '@/lib/branding/service-brands'

export function serviceFields(service: SupportedService | null) {
  return service ? [...service.required_env, ...service.optional_env] : []
}

export function ServiceIconBox({ serviceKey }: { serviceKey: string }) {
  const [imageFailed, setImageFailed] = useState(false)
  const known = isServiceKey(serviceKey) ? serviceKey : null
  const brand = known ? SERVICE_BRANDS[known] : SERVICE_BRAND_FALLBACK
  const logo = !imageFailed && known ? SERVICE_LOGOS[known] : null
  const svg = known ? SERVICE_SVG_FALLBACKS[known] : undefined

  return (
    <div
      className="flex size-9 shrink-0 items-center justify-center rounded-lg"
      style={{
        background: 'var(--aurora-control-surface)',
        border: `2px solid ${brand}`,
        boxShadow: `0 0 0 1px ${brand}33`,
      }}
    >
      {logo ? (
        <Image
          src={logo}
          alt=""
          className="size-5 object-contain"
          height={20}
          width={20}
          unoptimized
          onError={() => setImageFailed(true)}
        />
      ) : svg ? (
        <span
          className="block size-5"
          style={{ color: brand }}
          dangerouslySetInnerHTML={{ __html: svg.replace('fill="white"', `fill="${brand}"`) }}
        />
      ) : (
        <span className="text-xs font-bold" style={{ color: brand }}>
          {serviceKey[0]?.toUpperCase()}
        </span>
      )}
    </div>
  )
}
