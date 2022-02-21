import pymysql

from model.transaction import Transaction

limit: int = 1000


def create(db: pymysql.Connection, t: Transaction):
    db.begin()
    with db.cursor() as cursor:
        q = "select ifnull(sum(amount), 0) as total from transactions where user_id = %s for update"
        cursor.execute(q, (t.user_id,))
        res = cursor.fetchone()
        if res['total'] + t.amount >= limit:
            db.rollback()
            raise Exception("over limit")

        q = "insert into transactions(id, user_id, amount, description) values (%s, %s, %s, %s)"
        cursor.execute(q, (t.id, t.user_id, t.amount, t.description))

    db.commit()
