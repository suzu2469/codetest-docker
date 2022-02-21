from fastapi import FastAPI

from transaction import Transaction

app = FastAPI()


@app.get("/")
async def root():
    return {"message": "code test"}


@app.post("/transactions")
async def transactions(t: Transaction):
    return t