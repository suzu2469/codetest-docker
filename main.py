import pymysql
from fastapi import FastAPI, Depends, HTTPException, status

import repository.transaction
from dependency import get_db
from model.transaction import Transaction

app = FastAPI()


@app.get("/")
async def root():
    return {"message": "code test"}


@app.post("/transactions", status_code=status.HTTP_201_CREATED)
async def transactions(t: Transaction, db: pymysql.Connection = Depends(get_db)):
    try:
        repository.transaction.create(db, t)
    except:
        raise HTTPException(status_code=status.HTTP_402_PAYMENT_REQUIRED)
    return t