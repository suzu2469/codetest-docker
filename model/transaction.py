from pydantic import BaseModel


class Transaction(BaseModel):
    id: int
    user_id: int
    amount: int
    description: str
