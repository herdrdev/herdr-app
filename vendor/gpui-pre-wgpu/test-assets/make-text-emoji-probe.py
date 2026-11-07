from pathlib import Path
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen


def build(file, family, style, ps, weight, cmap):
    fb = FontBuilder(1000, isTTF=True)
    names = [".notdef", "space", "m", "box"]
    fb.setupGlyphOrder(names)
    glyphs = {}
    for name in names:
        p = TTGlyphPen(None)
        if name in ("m", "box"):
            p.moveTo((100, 0)); p.lineTo((100, 700)); p.lineTo((500, 700)); p.lineTo((500, 0)); p.closePath()
        glyphs[name] = p.glyph()
    fb.setupGlyf(glyphs)
    fb.setupHorizontalMetrics({n: (600, 0) for n in names})
    fb.setupHorizontalHeader(ascent=900, descent=-200)
    fb.setupCharacterMap({0x20: "space", 0x6D: "m", **cmap})
    fb.setupNameTable({"familyName": family, "styleName": style, "uniqueFontIdentifier": ps, "fullName": f"{family} {style}", "psName": ps})
    fb.setupOS2(sTypoAscender=900, sTypoDescender=-200, usWinAscent=900, usWinDescent=200, usWeightClass=weight)
    fb.setupPost(); fb.setupMaxp()
    fb.save(Path(__file__).with_name(file))


# Monochrome outlines for emoji, named like a font in cosmic-text's Unix
# fallback list ahead of Noto Color Emoji, in regular and bold.
emoji = {0x2764: "box", 0x1F579: "box", 0x1F680: "box"}
build("TextEmojiProbe.ttf", "Noto Sans Symbols2", "Regular", "NotoSansSymbols2-Regular", 400, emoji)
build("TextEmojiProbeBold.ttf", "Noto Sans Symbols2", "Bold", "NotoSansSymbols2-Bold", 700, emoji)
# A static bold text font without emoji: its weight differs from the emoji font's.
build("StaticBoldProbe.ttf", "Static Bold Probe", "Bold", "StaticBoldProbe-Bold", 700, {})
# A monochrome face with the color emoji font's family, style and weight.
build("TextEmojiDuplicate.ttf", "Noto Color Emoji", "Regular", "NotoColorEmojiMono", 400, emoji)
