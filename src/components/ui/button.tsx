import * as React from 'react';
import { Slot } from '@radix-ui/react-slot';
import { cva, type VariantProps } from 'class-variance-authority';
import { cn } from '@/lib/cn';

const buttonVariants = cva(
  'inline-flex items-center justify-center gap-1.5 whitespace-nowrap font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-hi focus-visible:ring-offset-2 focus-visible:ring-offset-bg disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0',
  {
    variants: {
      variant: {
        primary: 'bg-accent text-on-accent hover:bg-accent-hi shadow-sm',
        subtle: 'bg-surface-2 text-fg-2 hover:bg-hover border border-border',
        ghost: 'text-fg-3 hover:bg-hover hover:text-fg',
        danger: 'bg-danger text-on-accent hover:opacity-90 shadow-sm',
        link: 'text-accent underline-offset-4 hover:underline',
      },
      size: {
        sm: 'h-7 rounded-sm px-2.5 text-xs gap-1',
        md: 'h-9 rounded-md px-3.5 text-sm gap-1.5',
        lg: 'h-11 rounded-lg px-5 text-sm',
        icon: 'h-8 w-8 rounded-sm',
        'icon-sm': 'h-6 w-6 rounded-xs',
      },
    },
    defaultVariants: {
      variant: 'subtle',
      size: 'md',
    },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>, VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : 'button';
    return (
      <Comp className={cn(buttonVariants({ variant, size, className }))} ref={ref} {...props} />
    );
  },
);
Button.displayName = 'Button';

export { Button, buttonVariants };
