import pymysql
from fastapi import FastAPI, Depends

from dependency import get_db
from transaction import Transaction

app = FastAPI()


@app.get("/")
async def root():
    return {"message": "code test"}


@app.post("/transactions")
async def transactions(t: Transaction, db: pymysql.Connection = Depends(get_db)):
    return t