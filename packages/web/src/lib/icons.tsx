import {
  Baby,
  Camera,
  Film,
  Flower2,
  Folder,
  Heart,
  House,
  Images,
  Mountain,
  Music,
  Palette,
  PawPrint,
  Plane,
  Star,
  Sun,
  Trophy,
  type LucideIcon,
} from 'lucide-react';

export const LIBRARY_ICONS: Record<string, LucideIcon> = {
  images: Images,
  camera: Camera,
  film: Film,
  heart: Heart,
  mountain: Mountain,
  plane: Plane,
  palette: Palette,
  star: Star,
  music: Music,
  baby: Baby,
  house: House,
  sun: Sun,
  flower: Flower2,
  paw: PawPrint,
  trophy: Trophy,
  folder: Folder,
};

export function LibraryIcon({ name, className }: { name: string; className?: string }) {
  const I = LIBRARY_ICONS[name] ?? Images;
  return <I className={className} />;
}
