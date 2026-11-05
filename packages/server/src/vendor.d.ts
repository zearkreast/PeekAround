declare module 'heic-decode' {
  const decode: (opts: { buffer: Buffer | ArrayBuffer | Uint8Array }) => Promise<{ width: number; height: number; data: Uint8ClampedArray }>;
  export default decode;
}
declare module 'fontkit';
