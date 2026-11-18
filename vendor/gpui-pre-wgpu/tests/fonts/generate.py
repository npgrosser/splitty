"""Generate original rectangle-only font fixtures; no third-party artwork.

Regenerate with: uv run --with fonttools python tests/fonts/generate.py
Only the resulting tiny TTF files are needed to run Rust tests.
"""
from pathlib import Path
from tempfile import TemporaryDirectory

from fontTools.designspaceLib import AxisDescriptor, DesignSpaceDocument, SourceDescriptor
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.varLib import build

OUT = Path(__file__).parent


def master(path, family, weight, extra=None):
    width = 400 if weight == 400 else 700
    chars = {32: "space", 109: "m"}
    if extra:
        chars[extra] = "extra"
    glyphs = [".notdef", *chars.values()]
    builder = FontBuilder(1000, isTTF=True)
    builder.setupGlyphOrder(glyphs)
    builder.setupCharacterMap(chars)
    outlines = {}
    for name in glyphs:
        pen = TTGlyphPen(None)
        if name != "space":
            pen.moveTo((0, 0))
            pen.lineTo((width, 0))
            pen.lineTo((width, 700))
            pen.lineTo((0, 700))
            pen.closePath()
        outlines[name] = pen.glyph()
    builder.setupGlyf(outlines)
    builder.setupHorizontalMetrics({name: (width + 100, 0) for name in glyphs})
    builder.setupHorizontalHeader(ascent=800, descent=-200)
    builder.setupNameTable({"familyName": family, "styleName": "Regular" if weight == 400 else "Bold",
                           "uniqueFontIdentifier": f"{family}-{weight}", "fullName": family,
                           "psName": family.replace(" ", "")})
    builder.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800,
                     usWinDescent=200, usWeightClass=weight)
    builder.setupPost()
    builder.setupMaxp()
    builder.font["head"].created = builder.font["head"].modified = 3406620153
    builder.save(path)


def variable(name, extra=None):
    with TemporaryDirectory() as tmp:
        document = DesignSpaceDocument()
        axis = AxisDescriptor()
        axis.name, axis.tag = "Weight", "wght"
        axis.minimum, axis.default, axis.maximum = 400, 400, 700
        document.addAxis(axis)
        for weight in (400, 700):
            path = Path(tmp) / f"{weight}.ttf"
            master(path, name, weight, extra)
            source = SourceDescriptor()
            source.path = str(path)
            source.name = str(weight)
            source.location = {"Weight": weight}
            source.copyInfo = source.copyLib = source.copyFeatures = weight == 400
            document.addSource(source)
        font, _, _ = build(document)
        font.recalcTimestamp = False
        font.save(OUT / ("fallback.ttf" if extra else "variable.ttf"))


variable("Splitty Variable Test")
variable("Splitty Fallback Test", ord("Ω"))
master(OUT / "static.ttf", "Splitty Static Test", 400)
