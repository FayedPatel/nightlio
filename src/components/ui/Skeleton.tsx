import type { CSSVars } from '../../types/css';

interface SkeletonProps {
  height?: number | string;
  width?: number | string;
  radius?: number | string;
  style?: CSSVars;
}

const Skeleton = ({ height = 16, width = '100%', radius = 8, style = {} }: SkeletonProps) => (
  <div
    style={{
      height,
      width,
      borderRadius: radius,
  background: 'linear-gradient(90deg, var(--skeleton-a), var(--skeleton-b), var(--skeleton-a))',
      backgroundSize: '200% 100%',
      animation: 'skeleton 1.4s ease-in-out infinite',
      ...style,
    }}
  />
);

export default Skeleton;
