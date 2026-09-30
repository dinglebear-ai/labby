'use client'

import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { Field, FieldDescription, FieldGroup, FieldLabel } from '@/components/ui/field'
import type { ServiceConfig, SupportedService, SupportedServiceField } from '@/lib/types/gateway'
import { cn } from '@/lib/utils'

import { ServiceIconBox } from './gateway-service-fields'

const inputClassName =
  'border-aurora-border-strong bg-aurora-page-bg/80 shadow-[var(--aurora-highlight-medium)] placeholder:text-aurora-text-muted/70 hover:border-aurora-accent-primary/35 focus-visible:bg-aurora-control-surface'

interface GatewayLabServiceFormProps {
  supportedServices: SupportedService[]
  selectedService: string
  onSelectService: (service: string) => void
  serviceFields: SupportedServiceField[]
  serviceConfig?: ServiceConfig
  serviceValues: Record<string, string>
  onServiceValuesChange: (values: Record<string, string>) => void
  errors: Record<string, string>
  enableServer: boolean
  onEnableServerChange: (enabled: boolean) => void
}

export function GatewayLabServiceForm(props: GatewayLabServiceFormProps) {
  const {
    supportedServices, selectedService, onSelectService, serviceFields,
    serviceConfig, serviceValues, onServiceValuesChange, errors,
    enableServer, onEnableServerChange,
  } = props

  return (
    <div className="space-y-6">
      <FieldGroup>
        <Field>
          <div className="grid max-h-80 grid-cols-3 gap-2 overflow-y-auto pr-1 aurora-scrollbar sm:grid-cols-4">
            {supportedServices.map((service) => (
              <button
                key={service.key}
                type="button"
                onClick={() => onSelectService(service.key)}
                className={cn(
                  'flex flex-col items-center gap-1.5 rounded-aurora-2 border p-2 text-center transition-colors hover:border-primary/60 hover:bg-accent/30',
                  selectedService === service.key ? 'border-primary bg-primary/10' : 'border-aurora-border-strong bg-aurora-page-bg',
                )}
              >
                <ServiceIconBox serviceKey={service.key} />
                <div className="w-full min-w-0">
                  <p className="truncate text-xs font-medium leading-tight">{service.display_name}</p>
                  <p className="truncate text-[10px] text-aurora-text-muted">{service.category}</p>
                </div>
              </button>
            ))}
          </div>
          {errors.service && <p className="text-sm text-destructive">{errors.service}</p>}
        </Field>
      </FieldGroup>

      {selectedService && (
        <FieldGroup>
          {serviceFields.map((field) => {
            const configField = serviceConfig?.fields.find((item) => item.name === field.name)
            const hasStoredSecret = field.secret && configField?.present
            return (
              <Field key={field.name}>
                <FieldLabel htmlFor={field.name}>{field.name}</FieldLabel>
                <Input
                  id={field.name}
                  type={field.secret ? 'password' : 'text'}
                  value={serviceValues[field.name] ?? ''}
                  onChange={(event) => onServiceValuesChange({ ...serviceValues, [field.name]: event.target.value })}
                  placeholder={hasStoredSecret ? 'Leave blank to keep current value' : field.example}
                  className={cn(inputClassName, errors[field.name] && 'border-destructive')}
                />
                {errors[field.name] ? (
                  <p className="text-sm text-destructive">{errors[field.name]}</p>
                ) : (
                  <FieldDescription>{field.description}{hasStoredSecret ? ' Current secret is already configured.' : ''}</FieldDescription>
                )}
              </Field>
            )
          })}
        </FieldGroup>
      )}

      <div className="flex items-center justify-between rounded-lg border p-4">
        <div className="space-y-0.5">
          <Label htmlFor="enable-virtual-server" className="font-medium">Enable server</Label>
          <p className="text-sm text-aurora-text-muted">Save canonical service config and expose this Lab service as a visible server.</p>
        </div>
        <Switch id="enable-virtual-server" checked={enableServer} onCheckedChange={onEnableServerChange} />
      </div>
    </div>
  )
}
