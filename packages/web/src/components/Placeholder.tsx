import { Hammer } from 'lucide-react';
import { EmptyState } from './ui';

export function Placeholder({ title }: { title: string }) {
  return <EmptyState icon={<Hammer />} title={title} body="…" />;
}
