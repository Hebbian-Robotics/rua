from pydantic import BaseModel, Field


class Position(BaseModel):
    line: int
    column: int


class ByteOffset(BaseModel):
    start: int
    end: int


class CommentRange(BaseModel):
    start: Position
    end: Position
    byteOffset: ByteOffset


class Comment(BaseModel):
    file: str
    ruleId: str
    text: str
    language: str
    range: CommentRange


class Capture(BaseModel):
    text: str


class Captures(BaseModel):
    single: dict[str, Capture] = Field(default_factory=dict)
    multi: dict[str, list[Capture]] = Field(default_factory=dict)


class SyntaxMatch(Comment):
    metaVariables: Captures = Field(default_factory=Captures)
