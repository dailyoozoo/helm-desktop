import * as React from 'react';
import { cva, type VariantProps } from 'class-variance-authority';
import { cn } from '@/lib/cn';

const badgeVariants = cva(
  'inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[10px] font-medium transition-colors',
  {
    variants: {
      variant: {
        default: 'border-transparent bg-accent-soft text-accent-hi',
        secondary: 'border-transparent bg-surface-2 text-fg-3',
        success: 'border-transparent bg-success-soft text-success',
        warn: 'border-transparent bg-warn-soft text-warn',
        danger: 'border-transparent bg-danger-soft text-danger',
        outline: 'border-border text-fg-3',
      },
    },
    defaultVariants: { variant: 'default' },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>, VariantProps<typeof badgeVariants> {}

function Badge({ className, variant, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ variant }), className)} {...props} />;
}

export { Badge, badgeVariants };
