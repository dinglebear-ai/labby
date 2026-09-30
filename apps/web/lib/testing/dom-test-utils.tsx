import React from 'react'
import { act } from 'react'
import { createRoot } from 'react-dom/client'

import { installTestDom } from './dom-install.ts'

export { installTestDom }

export async function renderClient(element: React.ReactElement) {
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)

  await act(async () => {
    root.render(element)
  })

  return {
    container,
    rerender: async (next: React.ReactElement) => {
      await act(async () => {
        root.render(next)
      })
    },
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}
