# Crafted metadata test files for the pa-meta parity tests (EXIF / XMP / container edge cases).
# usage: python3 -I craft.py <outdir>
import io, os, struct, sys, json, zlib
from PIL import Image

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
os.makedirs(os.path.join(out, 'takeout'), exist_ok=True)

BYTE, ASCII, SHORT, LONG, RATIONAL, SBYTE, UNDEF, SSHORT, SLONG, SRATIONAL, FLOAT, DOUBLE = 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12
SIZES = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 1, 8: 2, 9: 4, 10: 8, 11: 4, 12: 8}


def enc_vals(typ, vals, e):
    if typ == ASCII:
        return vals if isinstance(vals, bytes) else vals.encode('utf-8') + b'\0'
    if typ in (BYTE, UNDEF):
        return bytes(vals)
    b = b''
    for v in vals:
        if typ == SHORT: b += struct.pack(e + 'H', v)
        elif typ == SSHORT: b += struct.pack(e + 'h', v)
        elif typ == LONG: b += struct.pack(e + 'I', v)
        elif typ == SLONG: b += struct.pack(e + 'i', v)
        elif typ == RATIONAL: b += struct.pack(e + 'II', *v)
        elif typ == SRATIONAL: b += struct.pack(e + 'ii', *v)
        elif typ == FLOAT: b += struct.pack(e + 'f', v)
        elif typ == DOUBLE: b += struct.pack(e + 'd', v)
        else: b += struct.pack(e + 'I', v)
    return b


def tiff(ifd0, exif=None, gps=None, le=True, count_override=None):
    """build a TIFF block. ifd*: list of (tag, type, values[, count])"""
    e = '<' if le else '>'
    ifd0 = list(ifd0)
    if exif is not None: ifd0.append((0x8769, LONG, [0]))
    if gps is not None: ifd0.append((0x8825, LONG, [0]))
    blocks = [('ifd0', ifd0)] + ([('exif', exif)] if exif is not None else []) + ([('gps', gps)] if gps is not None else [])
    # layout: header 8, then each IFD followed by its data
    offs = {}
    pos = 8
    for name, entries in blocks:
        offs[name] = pos
        size = 2 + 12 * len(entries) + 4
        data = sum((len(enc_vals(t[1], t[2], e)) + 1) & ~1 for t in entries if len(enc_vals(t[1], t[2], e)) > 4)
        pos += size + data
    out = bytearray(b'II*\0' if le else b'MM\0*') + struct.pack(e + 'I', 8)
    for name, entries in blocks:
        entries = sorted(entries, key=lambda t: t[0])
        start = offs[name]
        data_pos = start + 2 + 12 * len(entries) + 4
        body = bytearray(struct.pack(e + 'H', len(entries)))
        data = bytearray()
        for ent in entries:
            tag, typ, vals = ent[0], ent[1], ent[2]
            if tag == 0x8769: vals = [offs['exif']]
            if tag == 0x8825: vals = [offs['gps']]
            raw = enc_vals(typ, vals, e)
            cnt = ent[3] if len(ent) > 3 else (len(raw) // SIZES.get(typ, 1) if typ not in (ASCII,) else len(raw))
            if typ == ASCII: cnt = len(raw)
            body += struct.pack(e + 'HHI', tag, typ, cnt)
            if len(raw) <= 4:
                body += raw.ljust(4, b'\0')
            else:
                body += struct.pack(e + 'I', data_pos + len(data))
                data += raw + (b'\0' if len(raw) % 2 else b'')
        body += struct.pack(e + 'I', 0)
        out += body + data
    return bytes(out)


def jpeg(w=64, h=48, color=(120, 80, 40)):
    b = io.BytesIO()
    Image.new('RGB', (w, h), color).save(b, 'JPEG', quality=80)
    return b.getvalue()


def app(marker, payload):
    return bytes([0xFF, marker]) + struct.pack('>H', len(payload) + 2) + payload


def with_segments(jpg, segs):
    # insert after SOI (and any APP0)
    return jpg[:2] + b''.join(segs) + jpg[2:]


def exif_seg(t):
    return app(0xE1, b'Exif\0\0' + t)


def xmp_seg(x):
    return app(0xE1, b'http://ns.adobe.com/xap/1.0/\0' + x.encode('utf-8'))


def xmp_packet(desc_attrs='', inner=''):
    return ('<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
            f'<rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/" xmlns:tiff="http://ns.adobe.com/tiff/1.0/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" '
            f'xmlns:GPano="http://ns.google.com/photos/1.0/panorama/" xmlns:aux="http://ns.adobe.com/exif/1.0/aux/" xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" {desc_attrs}>{inner}</rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>')


def write(name, data):
    with open(os.path.join(out, name), 'wb') as f:
        f.write(data)


def r(v, d=1):
    return (int(round(v * d)), d)


def dms(v):
    a = abs(v)
    d = int(a)
    m = int((a - d) * 60)
    s = (a - d - m / 60) * 3600
    return [(d, 1), (m, 1), r(s, 100)]


std0 = [(0x010F, ASCII, 'NIKON CORPORATION'), (0x0110, ASCII, 'NIKON D750'), (0x0112, SHORT, [1])]
gps_ist = [(1, ASCII, 'N'), (2, RATIONAL, dms(41.0123)), (3, ASCII, 'E'), (4, RATIONAL, dms(28.9744))]

write('exif_full.jpg', with_segments(jpeg(), [exif_seg(tiff(
    std0[:2] + [(0x0112, SHORT, [6]), (0x0131, ASCII, 'Ver.1.00 '), (0x0132, ASCII, '2024:07:02 10:00:00')],
    [(0x9003, ASCII, '2024:07:01 23:30:00'), (0x9011, ASCII, '+03:00'), (0x9004, ASCII, '2024:07:01 23:30:00'), (0xA002, LONG, [6000]), (0xA003, LONG, [4000]),
     (0xA434, ASCII, '24.0-120.0 mm f/4.0'), (0x829A, RATIONAL, [(1, 250)]), (0x829D, RATIONAL, [(40, 10)]), (0x8827, SHORT, [200]), (0x9209, SHORT, [16]),
     (0x920A, RATIONAL, [(500, 10)]), (0xA405, SHORT, [50]), (0xA403, SHORT, [0])],
    gps_ist + [(5, BYTE, [0]), (6, RATIONAL, [(12345, 100)]), (7, RATIONAL, [(20, 1), (29, 1), (47, 1)]), (0x1D, ASCII, '2024:07:01')]))]))
write('exif_gps_time.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '2024:07:01 02:15:00')], gps_ist + [(7, RATIONAL, [(20, 1), (30, 1), (5, 1)]), (0x1D, ASCII, '2024:06:30')]))]))
write('exif_gps_time_bad.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '2024:07:01 02:15:00')], gps_ist + [(7, RATIONAL, [(20, 1), (30, 1), (5, 1)]), (0x1D, ASCII, '2024:06:20')]))]))
write('exif_createdate.jpg', with_segments(jpeg(), [exif_seg(tiff([(0x010F, ASCII, 'Apple'), (0x0110, ASCII, 'iPhone 13')], [(0x9004, ASCII, '2023:12:31 23:59:59'), (0x9012, ASCII, '-05:00'), (0xA434, ASCII, 'iPhone 13 front TrueDepth camera 2.71mm f/2.2')]))]))
write('exif_modifydate.jpg', with_segments(jpeg(), [exif_seg(tiff([(0x0132, ASCII, '2022:03:27 02:30:00')], [(0x9010, ASCII, '+0100')]))]))
write('exif_zero_date.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '0000:00:00 00:00:00'), (0x9004, ASCII, '2021:05:05 05:05:05')]))]))
write('exif_bigendian.jpg', with_segments(jpeg(), [exif_seg(tiff([(0x010F, ASCII, 'Canon'), (0x0110, ASCII, 'Canon EOS R5'), (0x0112, SHORT, [8])], [(0x9003, ASCII, '2020:02:29 12:00:00'), (0xA002, SHORT, [8192]), (0xA003, SHORT, [5464])], [(1, ASCII, 'S'), (2, RATIONAL, dms(33.8688)), (3, ASCII, 'E'), (4, RATIONAL, dms(151.2093))], le=False))]))
write('exif_nul_strings.jpg', with_segments(jpeg(), [exif_seg(tiff([(0x010F, ASCII, b'OLYMPUS IMAGING CORP.  \0\0\0'), (0x0110, ASCII, b'E-M10\0\0'), (0x0112, SHORT, [1])], [(0x9003, ASCII, '2019:01:01 00:00:00'), (0xA434, ASCII, b'\0\0')]))]))
write('exif_empty_make.jpg', with_segments(jpeg(), [exif_seg(tiff([(0x010F, ASCII, b'   \0'), (0x0110, ASCII, 'Model Only')], [(0x9003, ASCII, '2019:06:01 12:00:00')])),
                                                    xmp_seg(xmp_packet('tiff:Make="XmpMake" exif:DateTimeOriginal="2001-01-01T01:01:01" aux:Lens="XMP Lens"'))]))
write('exif_null_island.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '2018:01:01 10:00:00')], [(1, ASCII, 'N'), (2, RATIONAL, [(0, 1), (0, 1), (0, 1)]), (3, ASCII, 'E'), (4, RATIONAL, [(0, 1), (0, 1), (0, 1)])])),
                                                      xmp_seg(xmp_packet('exif:GPSLatitude="41,0.738N" exif:GPSLongitude="28,58.476E"'))]))
write('exif_gps_single.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '2018:01:01 10:00:00')], [(1, ASCII, 'N'), (2, RATIONAL, [(41, 1)]), (3, ASCII, 'E'), (4, RATIONAL, [(29, 1)])]))]))
write('exif_gps_noref.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [], [(2, RATIONAL, dms(52.52)), (4, RATIONAL, dms(13.405))]))]))
write('exif_gps_west.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '2017:08:08 08:08:08')], [(1, ASCII, 'N'), (2, RATIONAL, dms(40.7128)), (3, ASCII, 'W'), (4, RATIONAL, dms(74.006)), (7, RATIONAL, [(12, 1), (8, 1), (8, 1)]), (0x1D, ASCII, '2017:08:08')]))]))
write('exif_antimeridian.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '2016:01:01 00:00:00')], [(1, ASCII, 'S'), (2, RATIONAL, dms(16.5)), (3, ASCII, 'W'), (4, RATIONAL, dms(179.99))]))]))
write('exif_bad_type.jpg', with_segments(jpeg(), [exif_seg(tiff(std0 + [(0x0131, 99, [1, 2, 3, 4, 5, 6, 7, 8])], [(0x9003, ASCII, '2015:01:01 00:00:00')]))]))
write('exif_bad_exif_block.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '2015:01:01 00:00:00'), (0x9004, 99, [1, 2, 3, 4, 5, 6, 7, 8]), (0x9010, ASCII, '+02:00')]))]))
write('exif_utf8.jpg', with_segments(jpeg(), [exif_seg(tiff([(0x010F, ASCII, 'ÑIKON ÇORP'), (0x0110, ASCII, 'ÑIKON Ž1')], [(0x9003, ASCII, '2014:02:03 04:05:06.789'), (0x9011, ASCII, 'Z')]))]))
write('exif_dims_orient.jpg', with_segments(jpeg(32, 16), [exif_seg(tiff([(0x0112, SHORT, [5]), (0x0100, LONG, [111]), (0x0101, LONG, [222])], [(0xA002, SHORT, [3000]), (0xA003, SHORT, [2000])]))]))
write('exif_two_app1.jpg', with_segments(jpeg(), [exif_seg(tiff([(0x010F, ASCII, 'First')], [(0x9003, ASCII, '2013:01:01 00:00:00')])), exif_seg(tiff([(0x010F, ASCII, 'Second')], [(0x9003, ASCII, '2012:01:01 00:00:00')]))]))
write('exif_typed.jpg', with_segments(jpeg(), [exif_seg(tiff([(0x010F, ASCII, 'Sony'), (0x0110, ASCII, 'ILCE-7M3')], [(0x9003, ASCII, '2011:11:11 11:11:11'), (0x8827, SHORT, [100, 200]), (0x829A, RATIONAL, [(1, 0)]), (0xA002, SHORT, [4000, 1]), (0xA003, LONG, [3000])], [(1, ASCII, 'N'), (2, SRATIONAL, [(41, 1), (30, 1), (0, 1)]), (3, ASCII, 'E'), (4, DOUBLE, [29.5, 0.0, 0.0]), (6, SRATIONAL, [(-5, 1)])]))]))
write('exif_gps_string.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '2010:10:10 10:10:10')], [(1, ASCII, 'N'), (2, ASCII, '41.5'), (3, ASCII, 'W'), (4, ASCII, '8,30.0')]))]))
write('exif_offset_mismatch.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '2024:07:01 12:00:00+02:00'), (0x9011, ASCII, '+05:00')]))]))
write('exif_offsettime_only.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '2024:07:01 12:00:00'), (0x9010, ASCII, '-03:30')]))]))
write('no_meta.jpg', jpeg(80, 60))

# XMP only / mixed
write('xmp_only.jpg', with_segments(jpeg(), [xmp_seg(xmp_packet(
    'exif:GPSLatitude="41,0.738N" exif:GPSLongitude="28,58.476E" exif:DateTimeOriginal="2024-07-01T10:00:00+03:00" tiff:Make="Google" tiff:Model="Pixel 8" '
    'GPano:ProjectionType="equirectangular" GPano:UsePanoramaViewer="True" exif:PixelXDimension="8000" exif:PixelYDimension="4000"'))]))
write('xmp_elements.jpg', with_segments(jpeg(), [xmp_seg(xmp_packet('', '<exif:GPSLatitude>33,51,54.5S</exif:GPSLatitude><exif:GPSLongitude>151,12,33.5E</exif:GPSLongitude>'
    '<xmp:CreateDate>2023-03-04T05:06:07.5Z</xmp:CreateDate><GPano:UsePanoramaViewer>true</GPano:UsePanoramaViewer><aux:Lens>Some Lens</aux:Lens>'
    '<tiff:Make><rdf:Alt><rdf:li xml:lang="x-default">AltMake</rdf:li></rdf:Alt></tiff:Make><tiff:Orientation>6</tiff:Orientation>'))]))
write('xmp_conflict.jpg', with_segments(jpeg(), [exif_seg(tiff(std0, [(0x9003, ASCII, '2020:01:01 00:00:00')])),
    xmp_seg(xmp_packet('exif:DateTimeOriginal="1999-09-09T09:09:09+09:00" tiff:Make="XMPMAKE" exif:OffsetTimeOriginal="+09:00" GPano:ProjectionType="cylindrical"'))]))
write('xmp_first.jpg', with_segments(jpeg(), [xmp_seg(xmp_packet('tiff:Model="XmpModel" exif:GPSLatitude="10.5" exif:GPSLatitudeRef="S" exif:GPSLongitude="20.25" exif:GPSLongitudeRef="W"')),
    exif_seg(tiff([(0x010F, ASCII, 'ExifMake')], [(0x9003, ASCII, '2009:09:09 09:09:09')]))]))
write('xmp_photoshop.jpg', with_segments(jpeg(), [xmp_seg(xmp_packet('photoshop:DateCreated="2008-08-08" xmp:ModifyDate="2008-08-09T10:00:00-04:00" GPano:ProjectionType="Equirectangular"'))]))
write('xmp_gps_dms.jpg', with_segments(jpeg(), [xmp_seg(xmp_packet('exif:GPSLatitude="41°0\'44.28&quot;N" exif:GPSLongitude="28 58 28.5 E" exif:GPSDateStamp="2007:07:07" exif:GPSTimeStamp="2007-07-07T07:07:07Z" exif:DateTimeOriginal="2007-07-07T10:07:07"'))]))
write('xmp_nested_seq.jpg', with_segments(jpeg(), [xmp_seg(xmp_packet('', '<dc:subject xmlns:dc="http://purl.org/dc/elements/1.1/"><rdf:Bag><rdf:li>a</rdf:li><rdf:li>b</rdf:li></rdf:Bag></dc:subject>'
    '<exif:ISOSpeedRatings><rdf:Seq><rdf:li>400</rdf:li></rdf:Seq></exif:ISOSpeedRatings><GPano:ProjectionType>equirectangular</GPano:ProjectionType>'))]))

# PNG: eXIf, iTXt XMP, tEXt
def png_chunk(t, data):
    return struct.pack('>I', len(data)) + t + data + struct.pack('>I', zlib.crc32(t + data) & 0xffffffff)


def png(w, h, extra=b'', before_idat=True):
    b = io.BytesIO()
    Image.new('RGB', (w, h), (10, 200, 30)).save(b, 'PNG')
    d = b.getvalue()
    i = d.index(b'IDAT') - 4
    return d[:i] + extra + d[i:]


write('png_exif.png', png(1080, 2340, png_chunk(b'eXIf', tiff([(0x010F, ASCII, 'Google'), (0x0110, ASCII, 'Pixel 7'), (0x0112, SHORT, [6])], [(0x9003, ASCII, '2022:02:22 22:22:22')], gps_ist))))
write('png_xmp.png', png(1080, 2340, png_chunk(b'iTXt', b'XML:com.adobe.xmp\0\0\0\0\0' + xmp_packet('exif:DateTimeOriginal="2021-01-21T21:21:21" exif:GPSLatitude="48,51.396N" exif:GPSLongitude="2,21.132E"').encode())))
write('png_text.png', png(1440, 3200, png_chunk(b'tEXt', b'Software\0Some App')))
write('png_plain_wide.png', png(3000, 1000))

# WebP with EXIF orientation (exifr cannot read WebP; sharp applies the orientation to the size)
im = Image.new('RGB', (40, 20), (1, 2, 3))
exif = Image.Exif()
exif[0x0112] = 6
exif[0x010F] = 'WebpMake'
b = io.BytesIO(); im.save(b, 'WEBP', exif=exif.tobytes()); write('webp_exif.webp', b.getvalue())
frames = [Image.new('RGB', (30, 20), (i * 40, 0, 0)) for i in range(3)]
b = io.BytesIO(); frames[0].save(b, 'WEBP', save_all=True, append_images=frames[1:], duration=100, loop=0); write('anim.webp', b.getvalue())
b = io.BytesIO(); frames[0].save(b, 'GIF', save_all=True, append_images=frames[1:], duration=100, loop=0); write('anim.gif', b.getvalue())
b = io.BytesIO(); frames[0].save(b, 'PNG', save_all=True, append_images=frames[1:], duration=100, loop=0); write('anim.apng', b.getvalue())
b = io.BytesIO(); frames[0].save(b, 'PNG', save_all=True, append_images=frames[1:], duration=100, loop=0); write('anim_apng.png', b.getvalue())
b = io.BytesIO(); frames[0].save(b, 'GIF'); write('still.gif', b.getvalue())
b = io.BytesIO(); frames[0].save(b, 'WEBP'); write('still.webp', b.getvalue())
# TIFF: multipage + EXIF
b = io.BytesIO(); frames[0].save(b, 'TIFF', save_all=True, append_images=frames[1:]); write('multi.tiff', b.getvalue())
ex = Image.Exif(); ex[0x010F] = 'TiffMake'; ex[0x0110] = 'TiffModel'; ex[0x0132] = '2006:06:06 06:06:06'; ex[0x0112] = 8
b = io.BytesIO(); Image.new('RGB', (50, 30)).save(b, 'TIFF', exif=ex.tobytes()); write('exif.tif', b.getvalue())
# BMP / ICO / SVG
b = io.BytesIO(); Image.new('RGB', (13, 7)).save(b, 'BMP'); write('small.bmp', b.getvalue())
b = io.BytesIO(); Image.new('RGBA', (32, 32)).save(b, 'ICO', sizes=[(32, 32), (16, 16)]); write('icon.ico', b.getvalue())
write('a.svg', b'<svg xmlns="http://www.w3.org/2000/svg" width="300" height="150"><rect width="10" height="10"/></svg>')
write('viewbox.svg', b'<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 480"><circle r="5"/></svg>')
write('units.svg', b'<svg xmlns="http://www.w3.org/2000/svg" width="2in" height="10mm" viewBox="0 0 1 1"/>')
write('broken.jpg', b'\xff\xd8\xff\xe1\x00\x10Exif\0\0II*\0\x08\0\0\0')
write('notimage.png', b'hello world')
write('JPEG_as.png', jpeg(70, 50))

# Google Takeout sidecars
t = os.path.join('takeout')
write(f'{t}/IMG_T1.jpg', jpeg(20, 20))
write(f'{t}/IMG_T1.jpg.json', json.dumps({'title': 'IMG_T1.jpg', 'photoTakenTime': {'timestamp': '1562235010'}, 'geoData': {'latitude': 37.9715, 'longitude': 23.7267}}).encode())
write(f'{t}/IMG_T2(1).jpg', jpeg(20, 20))
write(f'{t}/IMG_T2.jpg(1).json', json.dumps({'photoTakenTime': {'timestamp': '1600000000'}, 'geoData': {'latitude': 0.0, 'longitude': 0.0}, 'geoDataExif': {'latitude': 41.123, 'longitude': 29.456}}).encode())
write(f'{t}/IMG_T3.jpg', with_segments(jpeg(20, 20), [exif_seg(tiff(std0, [(0x9003, ASCII, '2019:07:07 07:07:07')]))]))
write(f'{t}/IMG_T3.jpg.supplemental-metadata.json', json.dumps({'photoTakenTime': {'timestamp': '1111111111'}, 'geoData': {'latitude': 52.52, 'longitude': 13.405}}).encode())
write(f'{t}/IMG_T4-edited.jpg', jpeg(20, 20))
write(f'{t}/IMG_T4.jpg.json', json.dumps({'photoTakenTime': {'timestamp': '1234567890'}}).encode())
write(f'{t}/Screenshot_20190101-120000_Some Long App Name.png', png(100, 200))
write(f'{t}/Screenshot_20190101-120000_Some Long App Na.json', json.dumps({'photoTakenTime': {'timestamp': '1546344000'}}).encode())
write(f'{t}/IMG_T5.jpg', with_segments(jpeg(20, 20), [exif_seg(tiff(std0, [(0x9003, ASCII, '2019:07:07 07:07:07')], gps_ist))]))
write(f'{t}/IMG_T5.jpg.json', json.dumps({'photoTakenTime': {'timestamp': '1111111111'}, 'geoData': {'latitude': 1.5, 'longitude': 1.5}}).encode())
write(f'{t}/IMG_T6.jpg', jpeg(20, 20))
write(f'{t}/IMG_T6.jpg.json', b'{not json')
write(f'{t}/IMG_T7.HEIC.json', json.dumps({'photoTakenTime': {'timestamp': '1300000000'}, 'geoData': {'latitude': -33.8688, 'longitude': 151.2093}}).encode())
print('ok')
